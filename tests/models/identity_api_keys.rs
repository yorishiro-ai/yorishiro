use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro_core::app::App;
use yorishiro_core::models::_entities::{identity_tenants, identity_workspaces};
use yorishiro_core::models::identity_api_keys::Entity as ApiKeys;
use yorishiro_core::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro_core::services::auth::ApiKeyScope;

async fn setup_workspace(ctx: &loco_rs::app::AppContext) -> uuid::Uuid {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("api-key-test".into()),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");
    let workspace = identity_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set("main".into()),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert workspace");
    workspace.id
}

/// `list_for_workspace` must return every key issued for the workspace, oldest first, and never leak into another workspace's listing.
#[tokio::test]
#[serial]
async fn list_for_workspace_returns_only_that_workspaces_keys_oldest_first() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
        let workspace_id = setup_workspace(&ctx).await;
        let other_workspace_id = setup_workspace(&ctx).await;

        let first = ApiKeys::create_api_key(&ctx.db, workspace_id, ApiKeyScope::Read, None)
            .await
            .expect("create first key");
        let second = ApiKeys::create_api_key(&ctx.db, workspace_id, ApiKeyScope::Write, None)
            .await
            .expect("create second key");
        ApiKeys::create_api_key(&ctx.db, other_workspace_id, ApiKeyScope::Migration, None)
            .await
            .expect("create key in other workspace");

        let keys = ApiKeys::list_for_workspace(&ctx.db, workspace_id)
            .await
            .expect("list_for_workspace");

        assert_eq!(keys.len(), 2, "keys: {keys:?}");
        assert_eq!(keys[0].id, first.id, "oldest first");
        assert_eq!(keys[1].id, second.id);
        assert!(
            keys.iter().all(|k| k.workspace_id == Some(workspace_id)),
            "no cross-workspace leakage: {keys:?}"
        );

        crate::requests::close_app_pools(&ctx).await;
    })
    .await;
}

/// `revoke` deletes the row so authentication (a lookup on every request) can no longer find it, and a second revoke of the same id must report not-found rather than succeeding silently.
#[tokio::test]
#[serial]
async fn revoke_deletes_the_key_and_a_second_revoke_reports_not_found() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
        let workspace_id = setup_workspace(&ctx).await;
        let created = ApiKeys::create_api_key(&ctx.db, workspace_id, ApiKeyScope::Read, None)
            .await
            .expect("create key");

        ApiKeys::revoke(&ctx.db, created.id)
            .await
            .expect("first revoke succeeds");

        let keys = ApiKeys::list_for_workspace(&ctx.db, workspace_id)
            .await
            .expect("list_for_workspace");
        assert!(keys.is_empty(), "revoked key must be gone: {keys:?}");

        let result = ApiKeys::revoke(&ctx.db, created.id).await;
        assert!(
            matches!(
                result,
                Err(yorishiro_core::error::YorishiroError::NotFound { .. })
            ),
            "result: {result:?}"
        );

        crate::requests::close_app_pools(&ctx).await;
    })
    .await;
}
