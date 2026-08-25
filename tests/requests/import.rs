use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro_core::app::App;
use yorishiro_core::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro_core::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro_core::models::tenancy::{self, MembershipRole};
use yorishiro_core::services::auth::ApiKeyScope;

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

/// A schema line is only present in the import file when the exporting workspace's own schema is
/// being restored. An entity referencing a schema that already exists in the destination workspace
/// (created directly, not via this import) has to fall through to a lookup by id instead, and that
/// lookup is memoized (`import::import_jsonl`'s `schema_name_by_id` cache) since a batch of entities
/// sharing one pre-existing schema is the common case.
///
/// Two entities against the same pre-existing schema exercises both the cache miss (first entity)
/// and the cache hit (second entity), and asserts both actually land with the right entity_type
/// rather than just checking the import didn't error.
#[tokio::test]
#[serial]
async fn import_resolves_a_pre_existing_schema_for_every_entity_line() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let Setup { key } = setup(&ctx).await;
        let auth = format!("Bearer {key}");

        let schema_response = request
            .post("/api/schemas")
            .add_header("Authorization", auth.clone())
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(schema_response.status_code(), 201);
        let schema_id = schema_response.json::<serde_json::Value>()["schema"]["id"]
            .as_str()
            .expect("schema id")
            .to_string();

        let body = format!(
            "{}\n{}\n",
            serde_json::json!({
                "kind": "entity",
                "record": {
                    "id": uuid::Uuid::new_v4(),
                    "workspace_id": uuid::Uuid::new_v4(),
                    "schema_id": schema_id,
                    "schema_version": 1,
                    "entity_type": "project",
                    "data": { "title": "first", "status": "active" },
                    "created_at": chrono::Utc::now(),
                    "updated_at": chrono::Utc::now(),
                    "created_by": null,
                    "updated_by": null,
                }
            }),
            serde_json::json!({
                "kind": "entity",
                "record": {
                    "id": uuid::Uuid::new_v4(),
                    "workspace_id": uuid::Uuid::new_v4(),
                    "schema_id": schema_id,
                    "schema_version": 1,
                    "entity_type": "project",
                    "data": { "title": "second", "status": "active" },
                    "created_at": chrono::Utc::now(),
                    "updated_at": chrono::Utc::now(),
                    "created_by": null,
                    "updated_by": null,
                }
            }),
        );

        let import_response = request
            .post("/api/import.jsonl")
            .add_header("Authorization", auth.clone())
            .text(body)
            .await;
        assert_eq!(
            import_response.status_code(),
            200,
            "response: {:?}",
            import_response.text()
        );
        let result: serde_json::Value = import_response.json();
        assert_eq!(result["entities"], 2, "result: {result:?}");
        assert_eq!(result["schemas"], 0, "result: {result:?}");

        let list_response = request
            .get("/api/entities?entity_type=project")
            .add_header("Authorization", auth)
            .await;
        assert_eq!(list_response.status_code(), 200);
        let items: Vec<serde_json::Value> = list_response.json();
        assert_eq!(items.len(), 2, "items: {items:?}");
        let titles: std::collections::BTreeSet<_> = items
            .iter()
            .map(|item| item["data"]["title"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            std::collections::BTreeSet::from(["first", "second"]),
            "both entities imported against the cached schema lookup, not just the first"
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}
