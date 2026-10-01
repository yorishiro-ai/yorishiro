use crate::requests::boot_request;
#[cfg(feature = "test-support")]
use chrono::{DateTime, Duration, Utc};
#[cfg(feature = "test-support")]
use sea_orm::ActiveModelTrait;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, TransactionTrait};
use std::sync::Arc;
use tokio::sync::Barrier;
use yorishiro::app::App;
use yorishiro::models::_entities::{tenant_tenants, workspace_workspaces};
use yorishiro::models::tenancy;

#[cfg(feature = "test-support")]
pub(crate) async fn assert_invitation_boundaries(db: &sea_orm::DatabaseConnection) {
    let txn = db.begin().await.expect("begin invite boundary transaction");
    let tenant = tenant_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set(format!("invite-boundary-{}", uuid::Uuid::new_v4())),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .expect("insert tenant");
    let now = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .expect("parse fixed timestamp")
        .with_timezone(&Utc);

    let (at_expiry, token) = tenancy::test_support::create_invite_at(
        &txn,
        tenant.id,
        "at-expiry@example.com",
        tenancy::MembershipRole::Member,
        Duration::seconds(1),
        now - Duration::seconds(1),
    )
    .await
    .expect("create at-expiry invite");
    assert_eq!(DateTime::<Utc>::from(at_expiry.expires_at), now);
    assert!(
        tenancy::test_support::redeem_invite_at(&txn, &token, now)
            .await
            .expect("redeem at-expiry invite")
            .is_none()
    );

    let (before_expiry, token) = tenancy::test_support::create_invite_at(
        &txn,
        tenant.id,
        "before-expiry@example.com",
        tenancy::MembershipRole::Member,
        Duration::seconds(1),
        now,
    )
    .await
    .expect("create before-expiry invite");
    assert_eq!(
        DateTime::<Utc>::from(before_expiry.expires_at),
        now + Duration::seconds(1)
    );
    assert!(
        tenancy::test_support::redeem_invite_at(&txn, &token, now)
            .await
            .expect("redeem before-expiry invite")
            .is_some()
    );

    let (after_expiry, token) = tenancy::test_support::create_invite_at(
        &txn,
        tenant.id,
        "after-expiry@example.com",
        tenancy::MembershipRole::Member,
        Duration::seconds(1),
        now,
    )
    .await
    .expect("create after-expiry invite");
    assert_eq!(
        DateTime::<Utc>::from(after_expiry.expires_at),
        now + Duration::seconds(1)
    );
    assert!(
        tenancy::test_support::redeem_invite_at(&txn, &token, now + Duration::seconds(2))
            .await
            .expect("redeem after-expiry invite")
            .is_none()
    );

    let ttl = Duration::hours(72);
    let (issued, _) = tenancy::test_support::create_invite_at(
        &txn,
        tenant.id,
        "ttl@example.com",
        tenancy::MembershipRole::Member,
        ttl,
        now,
    )
    .await
    .expect("create TTL invite");
    assert_eq!(DateTime::<Utc>::from(issued.expires_at), now + ttl);
    txn.rollback()
        .await
        .expect("rollback invite boundary transaction");
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn invite_expiry_boundaries_match_postgres_gt() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        assert_invitation_boundaries(&ctx.db).await;
    })
    .await;
}

/// Eight concurrent `create_workspace` calls against a tenant with one workspace slot left must produce exactly one workspace, not eight.
///
/// The racers go through a `Barrier` rather than `tokio::join!`: joined futures on a single-threaded runtime interleave only at their own await points and reliably let the first one finish its count-and-insert before the second starts, so the gap never opens and the test passes against the unfixed code, proving nothing.
/// Releasing all eight from a barrier on a multi-threaded runtime makes them contend for real, which is what makes a missing lock observable: without `db::lock_for_update` in `create_workspace`, every racer's `SELECT count(*)` reads the same pre-insert snapshot, all eight see a free slot, and all eight insert.
///
/// This is the gate `testing.md` requires: a deliberate violation, not a happy-path assertion. Reverting the `lock_for_update` call in `create_workspace` fails this test.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_create_workspace_cannot_exceed_the_cap() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        const RACERS: usize = 8;

        // A cap of 2 with one workspace already present leaves exactly one slot for eight racers to fight over.
        let tenant = tenant_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("race".into()),
            max_workspaces: sea_orm::ActiveValue::Set(Some(2)),
            ..Default::default()
        };
        let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
            .await
            .expect("insert tenant");

        let txn = ctx.db.begin().await.expect("begin seed txn");
        tenancy::create_workspace(&txn, tenant.id, "first", None, None, None)
            .await
            .expect("seed the tenant's first workspace");
        txn.commit().await.expect("commit seed");

        let barrier = Arc::new(Barrier::new(RACERS));
        let mut handles = Vec::with_capacity(RACERS);
        for i in 0..RACERS {
            let db = ctx.db.clone();
            let barrier = Arc::clone(&barrier);
            let tenant_id = tenant.id;
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                let txn = db.begin().await.expect("begin racer txn");
                let result = tenancy::create_workspace(
                    &txn,
                    tenant_id,
                    &format!("racer-{i}"),
                    None,
                    None,
                    None,
                )
                .await;
                match result {
                    Ok(_) => txn.commit().await.is_ok(),
                    Err(_) => {
                        txn.rollback().await.expect("rollback rejected racer");
                        false
                    }
                }
            }));
        }

        let mut succeeded = 0;
        for handle in handles {
            if handle.await.expect("racer task panicked") {
                succeeded += 1;
            }
        }

        assert_eq!(
            succeeded, 1,
            "exactly one racer should win the tenant's last workspace slot"
        );

        // The count is the assertion that actually matters: a racer could in principle return `Ok` without its row surviving, and the cap exists to bound rows, not return values.
        let total = workspace_workspaces::Entity::find()
            .filter(workspace_workspaces::Column::TenantId.eq(tenant.id))
            .count(&ctx.db)
            .await
            .expect("count workspaces");
        assert_eq!(
            total, 2,
            "the tenant must never hold more than max_workspaces"
        );
    })
    .await;
}
