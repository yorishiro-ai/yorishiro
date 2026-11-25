use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant, issue_api_key};

use super::*;

#[tokio::test]
async fn schema_fork_tools_match_rest_and_enforce_workspace_scope() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_admin_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let session = initialize(&request).await;
        let source = request.post("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&json!({"template_id":"task-management"})).await;
        source.assert_status(StatusCode::CREATED);
        let source: Value = source.json();
        let target = request.post("/api/workspaces").add_header("Authorization", format!("Bearer {tenant_admin_key}")).json(&json!({"name":"fork target"})).await;
        target.assert_status(StatusCode::CREATED);
        let target: Value = target.json();
        let workspace_id = target["id"].as_str().unwrap().parse().unwrap();
        let key = issue_api_key(&ctx, workspace_id, tenant.owner_id, false).await;
        let read_key = issue_api_key(&ctx, workspace_id, crate::requests::fixtures::create_iam_reader(&ctx, tenant.tenant_id).await, false).await;
        let input = json!({"source_workspace_id":tenant.workspace_id,"source_schema_id":source["schema"]["id"]});
        let denied = mcp_call(&request, &session, &read_key, 2, "tools/call", json!({"name":"create_schema_fork","arguments":input})).await;
        assert_eq!(denied["result"]["isError"], true);
        let created = mcp_call(&request, &session, &key, 3, "tools/call", json!({"name":"create_schema_fork","arguments":input})).await;
        let fork = tool_result_json(&created);
        let id = fork["id"].as_str().unwrap();
        let rest = request.get(&format!("/api/schema-forks/{id}")).add_header("Authorization", format!("Bearer {key}")).await;
        rest.assert_status_ok();
        assert_eq!(fork, rest.json::<Value>());
        let listed = mcp_call(&request, &session, &read_key, 4, "tools/call", json!({"name":"list_schema_forks","arguments":{}})).await;
        assert_eq!(tool_result_json(&listed), json!([fork.clone()]));
        let foreign = mcp_call(&request, &session, &tenant.api_key, 5, "tools/call", json!({"name":"get_schema_fork","arguments":{"fork_id":id}})).await;
        assert_eq!(foreign["result"]["isError"], true);
        let stale = mcp_call(&request, &session, &key, 6, "tools/call", json!({"name":"update_schema_fork","arguments":{"fork_id":id,"definition":fork["definition"],"expected_fork_schema_id":Uuid::new_v4()}})).await;
        assert_eq!(stale["result"]["isError"], true);
        let updated = mcp_call(&request, &session, &key, 7, "tools/call", json!({"name":"update_schema_fork","arguments":{"fork_id":id,"definition":fork["definition"],"expected_fork_schema_id":fork["fork_schema_id"]}})).await;
        assert_eq!(tool_result_json(&updated)["id"], fork["id"]);
        let denied = mcp_call(&request, &session, &read_key, 8, "tools/call", json!({"name":"delete_schema_fork","arguments":{"fork_id":id}})).await;
        assert_eq!(denied["result"]["isError"], true);
        let deleted = mcp_call(&request, &session, &key, 9, "tools/call", json!({"name":"delete_schema_fork","arguments":{"fork_id":id}})).await;
        assert_eq!(tool_result_json(&deleted)["deleted"], true);
        request.get(&format!("/api/schema-forks/{id}")).add_header("Authorization", format!("Bearer {key}")).await.assert_status(StatusCode::NOT_FOUND);
        request.get(&format!("/api/schemas/{}", source["schema"]["id"].as_str().unwrap())).add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_status_ok();
    }).await;
}
