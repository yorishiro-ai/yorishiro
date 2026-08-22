use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro_core::app::App;
use yorishiro_core::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro_core::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro_core::models::tenancy::{self, MembershipRole};
use yorishiro_core::services::auth::ApiKeyScope;

async fn setup(ctx: &loco_rs::app::AppContext) -> String {
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
    identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Schema,
        Some(owner.id),
    )
    .await
    .expect("issue key")
    .plaintext
}

#[tokio::test]
#[serial]
async fn create_schema_from_a_builtin_template() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let key = setup(&ctx).await;

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

        super::close_app_pools(&ctx).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn create_schema_rejects_an_unknown_template_id() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let key = setup(&ctx).await;

        let response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "template_id": "no-such-template" }))
            .await;
        assert_eq!(
            response.status_code(),
            404,
            "response: {:?}",
            response.text()
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn list_templates_and_get_template_over_rest() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let key = setup(&ctx).await;

        let response = request
            .get("/api/templates")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(response.status_code(), 200);
        let body: serde_json::Value = response.json();
        let ids: Vec<&str> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"task-management"), "ids: {ids:?}");

        let response = request
            .get("/api/templates/task-management")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(response.status_code(), 200);
        let body: serde_json::Value = response.json();
        assert_eq!(body["name"], "task-management");

        super::close_app_pools(&ctx).await;
    })
    .await;
}
