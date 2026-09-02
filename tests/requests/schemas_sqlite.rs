/// SQLite counterpart for the schema creation request test.
///
/// Exercises the same schema creation flow as `schemas.rs` but boot against a
/// SQLite backend.
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::services::auth::ApiKeyScope;

struct Setup {
    key: String,
}

async fn setup(ctx: &loco_rs::app::AppContext) -> Setup {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("acme".into()),
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
    let owner = tenancy::create_user(&ctx.db, "owner@example.com", "hunter2-hunter2", None)
        .await
        .expect("create owner");
    tenancy::add_member(&ctx.db, tenant.id, owner.id, MembershipRole::Owner)
        .await
        .expect("add owner");
    let key = identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Schema,
        Some(owner.id),
        false,
    )
    .await
    .expect("issue key")
    .plaintext;
    Setup { key }
}

/// Creating a schema from a built-in template works on SQLite.
#[tokio::test]
#[serial]
#[ignore = "SQLite request tests: run with --include-ignored"]
async fn create_schema_from_a_builtin_template_sqlite() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir
        .path()
        .join(format!("yorishiro_schema_{}.sqlite3", uuid::Uuid::new_v4()));
    let db_path = db_path.to_str().expect("valid utf-8 path").to_string();
    super::request_with_create_sqlite::<App, _, _>(db_path.clone(), |request, ctx| async move {
        let Setup { key } = setup(&ctx).await;

        let response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(response.status_code(), 201);
        let body: serde_json::Value = response.json();
        assert_eq!(body["schema"]["name"], "task-management");
        assert!(
            body["schema"]["definition"]["entity_types"]["task"].is_object(),
            "body: {body}"
        );

        super::close_app_pools_sqlite(&ctx, &db_path).await;
    })
    .await;
}
