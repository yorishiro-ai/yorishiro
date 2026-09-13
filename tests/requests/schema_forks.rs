use super::boot_request;
use super::fixtures::{self, TenantArgs};
use sea_orm::ActiveValue;
use serial_test::serial;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::models::_entities::workspace_workspaces;
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::services::auth::ApiKeyScope;

#[tokio::test]
#[serial]
async fn schema_fork_crud_copy_and_follow_conflict() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant_id, source_workspace_id, owner_id, source_key) =
            fixtures::create_tenant_workspace_owner(
                &ctx,
                TenantArgs {
                    key_scope: ApiKeyScope::Schema,
                    ..Default::default()
                },
            )
            .await;
        let schema_response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {source_key}"))
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(schema_response.status_code(), 201);
        let schema: serde_json::Value = schema_response.json();
        let source_schema_id: Uuid = schema["schema"]["id"].as_str().unwrap().parse().unwrap();

        let target = workspace_workspaces::ActiveModel {
            tenant_id: ActiveValue::Set(tenant_id),
            name: ActiveValue::Set("target".into()),
            status: ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            ..Default::default()
        };
        let target = sea_orm::ActiveModelTrait::insert(target, &ctx.db)
            .await
            .unwrap();
        let target_key =
            fixtures::issue_api_key(&ctx, target.id, owner_id, ApiKeyScope::Schema, false).await;

        let created = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {target_key}"))
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace_id,
                "source_schema_id": source_schema_id
            }))
            .await;
        assert_eq!(created.status_code(), 201, "{}", created.text());
        let fork: serde_json::Value = created.json();
        assert_eq!(
            fork["source_schema_id"].as_str().unwrap(),
            source_schema_id.to_string()
        );
        assert_eq!(fork["customized"], false);
        let fork_id = fork["id"].as_str().unwrap();

        let duplicate = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {target_key}"))
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace_id,
                "source_schema_id": source_schema_id
            }))
            .await;
        assert_eq!(duplicate.status_code(), 409);

        let local = request
            .put(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .json(&serde_json::json!({
                "definition": fork["definition"],
                "expected_fork_schema_id": fork["fork_schema_id"]
            }))
            .await;
        assert_eq!(local.status_code(), 200, "{}", local.text());
        let local: serde_json::Value = local.json();
        assert_eq!(local["customized"], true);

        let follow = request
            .put(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .json(&serde_json::json!({
                "action": "follow",
                "expected_fork_schema_id": local["fork_schema_id"],
                "expected_source_schema_id": source_schema_id
            }))
            .await;
        assert_eq!(follow.status_code(), 409);

        let deleted = request
            .delete(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .await;
        assert_eq!(deleted.status_code(), 204);
    })
    .await;
}
