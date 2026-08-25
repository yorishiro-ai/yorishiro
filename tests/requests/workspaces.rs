use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro_core::app::App;
use yorishiro_core::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro_core::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro_core::models::tenancy::{self, MembershipRole};
use yorishiro_core::services::auth::ApiKeyScope;

async fn setup_tenant(ctx: &loco_rs::app::AppContext, name: &str) -> (uuid::Uuid, uuid::Uuid) {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set(name.to_string()),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");

    let workspace = identity_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set("main".to_string()),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert workspace");

    (tenant.id, workspace.id)
}

async fn issue_key_for(
    ctx: &loco_rs::app::AppContext,
    workspace_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> String {
    identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace_id,
        ApiKeyScope::Migration,
        Some(user_id),
        false,
    )
    .await
    .expect("issue key")
    .plaintext
}

#[tokio::test]
#[serial]
async fn owner_can_create_list_view_and_delete_workspaces() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let (tenant_id, main_id) = setup_tenant(&ctx, "acme").await;
        let owner = tenancy::create_user(&ctx.db, "owner@example.com", "hunter2-hunter2", None)
            .await
            .expect("create owner");
        tenancy::add_member(&ctx.db, tenant_id, owner.id, MembershipRole::Owner)
            .await
            .expect("add owner");
        let owner_key = issue_key_for(&ctx, main_id, owner.id).await;

        let response = request
            .post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({ "name": "staging" }))
            .await;
        assert_eq!(response.status_code(), 201);
        let body: serde_json::Value = response.json();
        assert_eq!(body["name"], "staging");
        assert_eq!(body["tenant_id"], tenant_id.to_string());
        let staging_id = body["id"].as_str().unwrap().to_string();

        let response = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(response.status_code(), 200);
        let body: serde_json::Value = response.json();
        let names: Vec<&str> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"main"));
        assert!(names.contains(&"staging"));

        let response = request
            .get(&format!("/api/workspaces/{staging_id}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(response.status_code(), 200);
        let body: serde_json::Value = response.json();
        assert_eq!(body["entity_count"], 0);
        assert_eq!(body["relation_count"], 0);
        assert_eq!(body["schema_count"], 0);

        let response = request
            .delete(&format!("/api/workspaces/{staging_id}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(response.status_code(), 204);

        let response = request
            .get(&format!("/api/workspaces/{staging_id}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(response.status_code(), 404);

        super::close_app_pools(&ctx).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn cannot_delete_a_tenants_only_workspace() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let (tenant_id, main_id) = setup_tenant(&ctx, "acme").await;
        let owner = tenancy::create_user(&ctx.db, "owner@example.com", "hunter2-hunter2", None)
            .await
            .expect("create owner");
        tenancy::add_member(&ctx.db, tenant_id, owner.id, MembershipRole::Owner)
            .await
            .expect("add owner");
        let owner_key = issue_key_for(&ctx, main_id, owner.id).await;

        let response = request
            .delete(&format!("/api/workspaces/{main_id}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(response.status_code(), 409);

        super::close_app_pools(&ctx).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn member_role_cannot_create_or_delete_workspaces() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let (tenant_id, main_id) = setup_tenant(&ctx, "acme").await;
        let member = tenancy::create_user(&ctx.db, "member@example.com", "hunter2-hunter2", None)
            .await
            .expect("create member");
        tenancy::add_member(&ctx.db, tenant_id, member.id, MembershipRole::Member)
            .await
            .expect("add member");
        let member_key = issue_key_for(&ctx, main_id, member.id).await;

        let response = request
            .post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .json(&serde_json::json!({ "name": "staging" }))
            .await;
        assert_eq!(response.status_code(), 403);

        let response = request
            .delete(&format!("/api/workspaces/{main_id}"))
            .add_header("Authorization", format!("Bearer {member_key}"))
            .await;
        assert_eq!(response.status_code(), 403);

        // A Member-role key can still list/view workspaces, just not create/delete them.
        let response = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .await;
        assert_eq!(response.status_code(), 200);

        super::close_app_pools(&ctx).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn workspaces_endpoints_require_authentication() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let response = request.get("/api/workspaces").await;
        assert_eq!(response.status_code(), 401);

        super::close_app_pools(&ctx).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn workspace_endpoints_enforce_tenant_isolation() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let (tenant_a, workspace_a) = setup_tenant(&ctx, "acme").await;
        let owner_a = tenancy::create_user(&ctx.db, "owner-a@example.com", "hunter2-hunter2", None)
            .await
            .expect("create owner a");
        tenancy::add_member(&ctx.db, tenant_a, owner_a.id, MembershipRole::Owner)
            .await
            .expect("add owner a");
        let owner_a_key = issue_key_for(&ctx, workspace_a, owner_a.id).await;

        let (_tenant_b, workspace_b) = setup_tenant(&ctx, "beta").await;

        // Tenant A's key must not be able to see or delete tenant B's workspace by guessing its id: identity_workspaces has no RLS of its own, so get_workspace_in_tenant's explicit tenant_id check is the only thing enforcing this boundary.
        let response = request
            .get(&format!("/api/workspaces/{workspace_b}"))
            .add_header("Authorization", format!("Bearer {owner_a_key}"))
            .await;
        assert_eq!(response.status_code(), 404);

        let response = request
            .delete(&format!("/api/workspaces/{workspace_b}"))
            .add_header("Authorization", format!("Bearer {owner_a_key}"))
            .await;
        assert_eq!(response.status_code(), 404);

        super::close_app_pools(&ctx).await;
    })
    .await;
}
