/// SQLite counterparts for workspace CRUD request tests.
///
/// These exercise the same workspace create/list/delete flow as `workspaces.rs`
/// but boot against a SQLite backend.
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::services::auth::ApiKeyScope;

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

/// An owner can create, list, view, and delete workspaces.
#[tokio::test]
#[serial]
async fn owner_can_create_list_view_and_delete_workspaces_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir
        .path()
        .join(format!("yorishiro_test_{}.sqlite3", uuid::Uuid::new_v4()));
    let db_path = db_path.to_str().expect("valid utf-8 path").to_string();
    super::request_with_create_sqlite::<App, _, _>(db_path.clone(), |request, ctx| async move {
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
    })
    .await;
}

/// Workspace endpoints require authentication.
#[tokio::test]
#[serial]
async fn workspaces_endpoints_require_authentication_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir
        .path()
        .join(format!("yorishiro_test_{}.sqlite3", uuid::Uuid::new_v4()));
    let db_path = db_path.to_str().expect("valid utf-8 path").to_string();
    super::request_with_create_sqlite::<App, _, _>(db_path.clone(), |request, ctx| async move {
        let response = request.get("/api/workspaces").await;
        assert_eq!(response.status_code(), 401);
    })
    .await;
}
