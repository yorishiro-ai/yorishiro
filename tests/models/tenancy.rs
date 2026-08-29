use loco_rs::testing::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, TransactionTrait};
use serial_test::serial;
use std::sync::Arc;
use tokio::sync::Barrier;
use yorishiro::app::App;
use yorishiro::models::_entities::{identity_tenants, identity_workspaces};
use yorishiro::models::tenancy;

/// Eight concurrent `create_workspace` calls against a tenant with one workspace slot left must produce exactly one workspace, not eight.
///
/// The racers go through a `Barrier` rather than `tokio::join!`: joined futures on a single-threaded runtime interleave only at their own await points and reliably let the first one finish its count-and-insert before the second starts, so the gap never opens and the test passes against the unfixed code, proving nothing.
/// Releasing all eight from a barrier on a multi-threaded runtime makes them contend for real, which is what makes a missing lock observable: without `db::lock_for_update` in `create_workspace`, every racer's `SELECT count(*)` reads the same pre-insert snapshot, all eight see a free slot, and all eight insert.
///
/// This is the gate `testing.md` requires: a deliberate violation, not a happy-path assertion. Reverting the `lock_for_update` call in `create_workspace` fails this test.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[serial]
async fn concurrent_create_workspace_cannot_exceed_the_cap() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
        const RACERS: usize = 8;

        // A cap of 2 with one workspace already present leaves exactly one slot for eight racers to fight over.
        let tenant = identity_tenants::ActiveModel {
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
                let created = tenancy::create_workspace(
                    &txn,
                    tenant_id,
                    &format!("racer-{i}"),
                    None,
                    None,
                    None,
                )
                .await
                .is_ok();
                // Committing only on success keeps a rejected racer from leaving a half-open transaction behind; the cap rejection is an `Err`, so there is nothing to commit.
                // A losing racer's commit can itself fail, so its outcome is folded into the returned bool rather than unwrapped: an `expect` here runs inside a spawned task, and panicking there aborts the whole test process instead of failing this assertion, which hides which check actually broke.
                created && txn.commit().await.is_ok()
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
        let total = identity_workspaces::Entity::find()
            .filter(identity_workspaces::Column::TenantId.eq(tenant.id))
            .count(&ctx.db)
            .await
            .expect("count workspaces");
        assert_eq!(
            total, 2,
            "the tenant must never hold more than max_workspaces"
        );

        crate::requests::close_app_pools(&ctx).await;
    })
    .await;
}
