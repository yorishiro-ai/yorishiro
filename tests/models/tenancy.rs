use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use chrono::{DateTime, Duration, Utc};
use sea_orm::ActiveModelTrait;
use sea_orm::TransactionTrait;
use std::sync::Arc;
use tokio::sync::Barrier;
use yorishiro::App;
use yorishiro::models::_entities::tenant_tenants;
use yorishiro::models::{tenancy, tenant_memberships, workspace_invites};

pub(crate) async fn assert_invitation_boundaries(
    db: &sea_orm::DatabaseConnection,
    tenant_id: uuid::Uuid,
) {
    let txn = db.begin().await.expect("begin invite boundary transaction");
    let now = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .expect("parse fixed timestamp")
        .with_timezone(&Utc);

    let (at_expiry, token) = workspace_invites::create_invite_at(
        &txn,
        tenant_id,
        &crate::requests::fixtures::unique_email("at-expiry"),
        tenant_memberships::MembershipRole::Member,
        Duration::seconds(1),
        now - Duration::seconds(1),
    )
    .await
    .expect("create at-expiry invite");
    assert_eq!(DateTime::<Utc>::from(at_expiry.expires_at), now);
    assert!(
        workspace_invites::redeem_invite_at(&txn, &token, now)
            .await
            .expect("redeem at-expiry invite")
            .is_none()
    );

    let (before_expiry, token) = workspace_invites::create_invite_at(
        &txn,
        tenant_id,
        &crate::requests::fixtures::unique_email("before-expiry"),
        tenant_memberships::MembershipRole::Member,
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
        workspace_invites::redeem_invite_at(&txn, &token, now)
            .await
            .expect("redeem before-expiry invite")
            .is_some()
    );

    let (after_expiry, token) = workspace_invites::create_invite_at(
        &txn,
        tenant_id,
        &crate::requests::fixtures::unique_email("after-expiry"),
        tenant_memberships::MembershipRole::Member,
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
        workspace_invites::redeem_invite_at(&txn, &token, now + Duration::seconds(2))
            .await
            .expect("redeem after-expiry invite")
            .is_none()
    );

    let ttl = Duration::hours(72);
    let (issued, _) = workspace_invites::create_invite_at(
        &txn,
        tenant_id,
        &crate::requests::fixtures::unique_email("ttl"),
        tenant_memberships::MembershipRole::Member,
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

#[tokio::test]
async fn invite_expiry_boundaries_match_postgres_gt() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        assert_invitation_boundaries(&ctx.db, tenant.tenant_id).await;
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
    crate::requests::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        const RACERS: usize = 8;

        // A cap of 2 with one workspace already present leaves exactly one slot for eight racers to fight over.
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        // No public operation changes workspace caps; inject the finite cap this race must enforce.
        tenant_tenants::ActiveModel {
            id: sea_orm::ActiveValue::Set(tenant.tenant_id),
            max_workspaces: sea_orm::ActiveValue::Set(Some(2)),
            ..Default::default()
        }
        .update(&ctx.db)
        .await
        .expect("set workspace cap");

        let barrier = Arc::new(Barrier::new(RACERS));
        let mut handles = Vec::with_capacity(RACERS);
        for i in 0..RACERS {
            let db = ctx.db.clone();
            let barrier = Arc::clone(&barrier);
            let tenant_id = tenant.tenant_id;
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                let txn = db.begin().await.expect("begin racer txn");
                let result =
                    tenancy::create_workspace(&txn, tenant_id, &format!("racer-{i}"), None, None)
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

        // This fixture's owner belongs only to the tenant whose final workspace set we check.
        let workspaces = tenancy::list_workspaces_for_user(&ctx.db, tenant.owner_id)
            .await
            .expect("list persisted workspaces");
        assert_eq!(
            workspaces.len(),
            2,
            "the tenant must never hold more than max_workspaces"
        );
    })
    .await;
}
