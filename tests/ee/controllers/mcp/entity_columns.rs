use serde_json::{Value, json};
use yorishiro::App;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

use super::*;

#[tokio::test]
async fn entity_column_tools_match_rest_and_enforce_write_scope() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let saved = mcp_call(&request, &session, &tenant.api_key, 2, "tools/call", json!({"name":"update_workspace_entity_columns","arguments":{"entity_type":"note","columns":["title","body"]}})).await;
        assert_eq!(tool_result_json(&saved), json!({"workspace_id":tenant.workspace_id,"entity_type":"note","columns":["title","body"]}));
        let listed = mcp_call(&request, &session, &tenant.api_key, 3, "tools/call", json!({"name":"list_workspace_entity_columns","arguments":{}})).await;
        let http_response = request.get("/api/workspace/entity-columns").add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
        http_response.assert_status_ok();
        assert_eq!(tool_result_json(&listed), http_response.json::<Value>());
        let invalid = mcp_call(&request, &session, &tenant.api_key, 4, "tools/call", json!({"name":"update_workspace_entity_columns","arguments":{"entity_type":"note","columns":["title","title"]}})).await;
        assert_eq!(invalid["result"]["isError"], true);
        let read_key = crate::requests::fixtures::create_key(&ctx.db, tenant.workspace_id, Some(crate::requests::fixtures::create_iam_reader(&ctx, tenant.tenant_id).await)).await.unwrap().plaintext;
        for (id, name, arguments) in [
            (5, "update_workspace_entity_columns", json!({"entity_type":"note","columns":[]})),
            (6, "delete_workspace_entity_columns", json!({"entity_type":"note"})),
        ] {
            let denied = mcp_call(&request, &session, &read_key, id, "tools/call", json!({"name":name,"arguments":arguments})).await;
            assert_eq!(denied["result"]["isError"], true);
        }
        let unchanged = mcp_call(&request, &session, &read_key, 7, "tools/call", json!({"name":"list_workspace_entity_columns","arguments":{}})).await;
        assert_eq!(tool_result_json(&unchanged), tool_result_json(&listed));
        let reset = mcp_call(&request, &session, &tenant.api_key, 8, "tools/call", json!({"name":"delete_workspace_entity_columns","arguments":{"entity_type":"note"}})).await;
        assert_eq!(tool_result_json(&reset)["reset"], true);
        request.get("/api/workspace/entity-columns").add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_json(&json!([]));
    }).await;
}
