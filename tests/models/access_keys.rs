use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use aws_lc_rs::digest::{SHA256, digest};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, TransactionTrait};
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

        let keys = AccessKeys::find()
            .filter(yorishiro::models::_entities::access_keys::Column::Id.eq(created.id))
            .all(&ctx.db)
            .await
            .expect("read the key");
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
