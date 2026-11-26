use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use aws_lc_rs::digest::{SHA256, digest};
use sea_orm::TransactionTrait;
use yorishiro::App;
use yorishiro::models::access_keys::Entity as AccessKeys;
use yorishiro::models::tenancy;

async fn empty_workspace(ctx: &loco_rs::app::AppContext, tenant_id: uuid::Uuid) -> uuid::Uuid {
    let txn = ctx.db.begin().await.expect("begin workspace creation");
    let workspace = tenancy::create_workspace(
        &txn,
        tenant_id,
        &format!("keys-{}", uuid::Uuid::new_v4().simple()),
        None,
        None,
    )
    .await
    .expect("create workspace without keys");
    txn.commit().await.expect("commit workspace creation");
    workspace.id
}

/// `list_for_workspace` must return every key issued for the workspace, oldest first, and never leak into another workspace's listing.
#[tokio::test]
async fn list_for_workspace_returns_only_that_workspaces_keys_oldest_first() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let workspace_id = empty_workspace(&ctx, tenant.tenant_id).await;
        let other_workspace_id = empty_workspace(&ctx, tenant.tenant_id).await;

        let first = AccessKeys::create_access_key(&ctx.db, workspace_id, None)
            .await
            .expect("create first key");
        let second = AccessKeys::create_access_key(&ctx.db, workspace_id, None)
            .await
            .expect("create second key");
        AccessKeys::create_access_key(&ctx.db, other_workspace_id, None)
            .await
            .expect("create key in other workspace");

        let keys = AccessKeys::list_for_workspace(
            &ctx.db,
            workspace_id,
            yorishiro::models::pagination::ListParams::default(),
        )
        .await
        .expect("list_for_workspace");

        assert_eq!(keys.len(), 2, "keys: {keys:?}");
        assert_eq!(keys[0].id, first.id, "oldest first");
        assert_eq!(keys[1].id, second.id);
        assert!(
            keys.iter().all(|k| k.workspace_id == Some(workspace_id)),
            "no cross-workspace leakage: {keys:?}"
        );
    })
    .await;
}

/// Revocation retains grant history while discarding the authenticating hash.
/// A second revoke reports not-found rather than silently changing the original revocation time.
#[tokio::test]
async fn revoke_retains_history_and_a_second_revoke_reports_not_found() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let workspace_id = empty_workspace(&ctx, tenant.tenant_id).await;
        let created = AccessKeys::create_access_key(&ctx.db, workspace_id, None)
            .await
            .expect("create key");

        AccessKeys::revoke(&ctx.db, created.id)
            .await
            .expect("first revoke succeeds");

        let keys = AccessKeys::list_for_workspace(
            &ctx.db,
            workspace_id,
            yorishiro::models::pagination::ListParams::default(),
        )
        .await
        .expect("list_for_workspace");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].id, created.id);
        assert!(keys[0].revoked_at.is_some());
        assert_ne!(
            keys[0].key_hash,
            digest(&SHA256, created.plaintext.as_bytes()).as_ref()
        );

        let result = AccessKeys::revoke(&ctx.db, created.id).await;
        assert!(
            matches!(
                result,
                Err(yorishiro::error::YorishiroError::NotFound { .. })
            ),
            "result: {result:?}"
        );
    })
    .await;
}
