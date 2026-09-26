use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::services::auth::ApiKeyScope;

use super::boot_request;
use super::fixtures::{self, TenantArgs, issue_api_key};

#[tokio::test]
async fn relations_are_created_listed_statused_exported_and_deleted_over_rest() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (_, workspace_id, owner_id, schema_key) = fixtures::create_tenant_workspace_owner(
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let write_key =
            issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Write, false).await;
        let read_key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, false).await;

        let schema = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {schema_key}"))
            .json(&json!({
                "name": "graph",
                "entity_types": {
                    "person": { "fields": { "name": { "type": "string", "required": true } } }
                },
                "relation_types": {
                    "knows": { "source": "person", "target": "person" }
                }
            }))
            .await;
        assert_eq!(
            schema.status_code(),
            StatusCode::CREATED,
            "{}",
            schema.text()
        );

        let create_entity = |name: &str| {
            request
                .post("/api/entities")
                .add_header("Authorization", format!("Bearer {write_key}"))
                .json(&json!({
                    "schema_name": "graph",
                    "entity_type": "person",
                    "data": { "name": name }
                }))
        };
        let alice_response = create_entity("alice").await;
        let bob_response = create_entity("bob").await;
        assert_eq!(alice_response.status_code(), StatusCode::CREATED);
        assert_eq!(bob_response.status_code(), StatusCode::CREATED);
        let alice: Value = alice_response.json();
        let bob: Value = bob_response.json();
        let alice_id: Uuid = alice["id"].as_str().unwrap().parse().unwrap();
        let bob_id: Uuid = bob["id"].as_str().unwrap().parse().unwrap();

        let created = request
            .post("/api/relations")
            .add_header("Authorization", format!("Bearer {write_key}"))
            .json(&json!({
                "source_id": alice_id,
                "target_id": bob_id,
                "relation_type": "knows",
                "properties": { "since": 2024 }
            }))
            .await;
        assert_eq!(
            created.status_code(),
            StatusCode::CREATED,
            "{}",
            created.text()
        );
        let relation: Value = created.json();
        let relation_id = relation["id"].as_str().unwrap().to_owned();
        assert_eq!(relation["status"], "active");
        assert_eq!(relation["properties"]["since"], 2024);

        let listed = request
            .get(&("/api/relations?source_id=".to_owned() + &alice_id.to_string()))
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        assert_eq!(listed.status_code(), StatusCode::OK, "{}", listed.text());
        let listed: Vec<Value> = listed.json();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"], relation_id);

        let deprecated = request
            .put(&format!("/api/relations/{relation_id}/status"))
            .add_header("Authorization", format!("Bearer {write_key}"))
            .json(&json!({ "status": "deprecated" }))
            .await;
        assert_eq!(
            deprecated.status_code(),
            StatusCode::OK,
            "{}",
            deprecated.text()
        );
        assert_eq!(deprecated.json::<Value>()["status"], "deprecated");

        let exported = request
            .get("/api/export.jsonl")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        assert_eq!(
            exported.status_code(),
            StatusCode::OK,
            "{}",
            exported.text()
        );
        assert_eq!(
            exported
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/x-ndjson")
        );
        let records: Vec<Value> = exported
            .text()
            .lines()
            .map(|line| serde_json::from_str(line).expect("export JSONL record"))
            .collect();
        assert!(records.iter().any(|record| record["kind"] == "schema"));
        assert!(records.iter().any(|record| record["kind"] == "entity"));
        assert!(records.iter().any(|record| {
            record["kind"] == "relation" && record["record"]["status"] == "deprecated"
        }));

        let deleted = request
            .delete(&format!("/api/relations/{relation_id}"))
            .add_header("Authorization", format!("Bearer {write_key}"))
            .await;
        assert_eq!(
            deleted.status_code(),
            StatusCode::NO_CONTENT,
            "{}",
            deleted.text()
        );

        let missing = request
            .get(&format!("/api/relations/{relation_id}"))
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        assert_eq!(
            missing.status_code(),
            StatusCode::NOT_FOUND,
            "{}",
            missing.text()
        );
    })
    .await;
}

#[tokio::test]
async fn relation_write_requires_write_scope_and_read_authentication() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (_, workspace_id, owner_id, schema_key) = fixtures::create_tenant_workspace_owner(
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let read_key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, false).await;
        let schema = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {schema_key}"))
            .json(&json!({
                "name": "graph",
                "entity_types": { "person": { "fields": {} } },
                "relation_types": { "knows": { "source": "person", "target": "person" } }
            }))
            .await;
        assert_eq!(schema.status_code(), StatusCode::CREATED);

        let denied = request
            .post("/api/relations")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .json(&json!({
                "source_id": Uuid::new_v4(),
                "target_id": Uuid::new_v4(),
                "relation_type": "knows"
            }))
            .await;
        assert_eq!(
            denied.status_code(),
            StatusCode::FORBIDDEN,
            "{}",
            denied.text()
        );

        let unauthenticated = request.get("/api/relations").await;
        assert_eq!(unauthenticated.status_code(), StatusCode::UNAUTHORIZED);
    })
    .await;
}
