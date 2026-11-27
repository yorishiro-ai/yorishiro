use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant, issue_access_key};

use super::*;

#[tokio::test]
async fn schema_archive_preserves_history_and_version_sequence() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let definition = json!({"name":"archive-history","entity_types":{"note":{"fields":{"title":{"type":"string"}}}}});
        let created = request.post("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&definition).await;
        created.assert_status(StatusCode::CREATED);
        let schema = created.json::<Value>()["schema"].clone();
        let id = schema["schema_id"].as_str().unwrap();
        let entity = request.post("/api/entities").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&json!({"schema_name":"archive-history","entity_type":"note","data":{"title":"retained"}})).await;
        entity.assert_status(StatusCode::CREATED);
        let entity_id = entity.json::<Value>()["entity_id"].as_str().unwrap().to_owned();
        let read_key = issue_access_key(&ctx, tenant.workspace_id, crate::requests::fixtures::create_iam_reader(&ctx, tenant.tenant_id).await).await;
        let denied = mcp_call(&request, &session, &read_key, 2, "tools/call", json!({"name":"archive_schema","arguments":{"schema_id":id}})).await;
        assert_eq!(denied["result"]["isError"], true);
        let archived = mcp_call(&request, &session, &tenant.api_key, 3, "tools/call", json!({"name":"archive_schema","arguments":{"schema_id":id}})).await;
        assert_eq!(tool_result_json(&archived)["status"], "archived");
        let retained = request.get(&format!("/api/entities/{entity_id}")).add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
        retained.assert_status_ok();
        assert_eq!(retained.json::<Value>()["data"]["title"], "retained");
        let again = request.post(&format!("/api/schemas/{id}:archive")).add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
        again.assert_status_ok();
        assert_eq!(again.json::<Value>(), tool_result_json(&archived));
        request.get("/api/schemas/active/archive-history").add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_status(StatusCode::NOT_FOUND);
        request.get(&format!("/api/schemas/{id}")).add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_status_ok();
        let next = request.post("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&definition).await;
        next.assert_status(StatusCode::CREATED);
        assert_eq!(next.json::<Value>()["schema"]["version"], 2);
        let missing = mcp_call(&request, &session, &tenant.api_key, 4, "tools/call", json!({"name":"archive_schema","arguments":{"schema_id":Uuid::new_v4()}})).await;
        assert_eq!(missing["result"]["isError"], true);
    }).await;
}

#[tokio::test]
async fn delete_schema_tool_enforces_scope_and_removes_an_archived_version() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let definition = json!({"name":"delete-history","entity_types":{"note":{"fields":{"title":{"type":"string"}}}}});
        let v1 = request.post("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&definition).await;
        v1.assert_status(StatusCode::CREATED);
        let v1_id = v1.json::<Value>()["schema"]["schema_id"].as_str().unwrap().to_owned();
        let v2 = request.post("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&definition).await;
        v2.assert_status(StatusCode::CREATED);
        let v2_id = v2.json::<Value>()["schema"]["schema_id"].as_str().unwrap().to_owned();
        let read_key = issue_access_key(&ctx, tenant.workspace_id, crate::requests::fixtures::create_iam_reader(&ctx, tenant.tenant_id).await).await;
        let denied = mcp_call(&request, &session, &read_key, 2, "tools/call", json!({"name":"delete_schema","arguments":{"schema_id":v1_id}})).await;
        assert_eq!(denied["result"]["isError"], true);
        request.post(&format!("/api/schemas/{v2_id}:archive")).add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_status_ok();
        let deleted = mcp_call(&request, &session, &tenant.api_key, 5, "tools/call", json!({"name":"delete_schema","arguments":{"schema_id":v2_id}})).await;
        assert_eq!(tool_result_json(&deleted)["deleted"], true);
        request.get(&format!("/api/schemas/{v2_id}")).add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_status(StatusCode::NOT_FOUND);
    }).await;
}
