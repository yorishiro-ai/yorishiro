use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

use super::*;

#[tokio::test]
async fn template_library_mutations_match_rest_and_remain_tenant_owned() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_admin_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let session = initialize(&request).await;
        let created = mcp_call(&request, &session, &tenant_admin_key, 2, "tools/call",
            json!({"name":"create_template_library_item","arguments":{"name":format!("mcp-template-{}",Uuid::now_v7()),"definition":{"name":"note","entity_types":{"note":{"fields":{"title":{"type":"string"}}}}}}})).await;
        let created = tool_result_json(&created);
        let id = created["id"].as_str().unwrap();
        let updated = mcp_call(&request, &session, &tenant_admin_key, 3, "tools/call",
            json!({"name":"update_template_library_item","arguments":{"id":id,"description":"updated through MCP"}})).await;
        let rest = request.get(&format!("/api/template-library/{id}")).add_header("Authorization", format!("Bearer {tenant_admin_key}")).await;
        rest.assert_status_ok();
        assert_eq!(tool_result_json(&updated), rest.json::<Value>());
        let fork = mcp_call(&request, &session, &tenant_admin_key, 4, "tools/call",
            json!({"name":"fork_template_library_item","arguments":{"id":id,"name":format!("fork-{}",Uuid::now_v7())}})).await;
        let fork = tool_result_json(&fork);
        assert_ne!(fork["id"], created["id"]);
        let service = yorishiro::models::api_keys::Entity::create_api_key(&ctx.db, tenant.workspace_id, ApiKeyScope::Migration, None, false).await.unwrap();
        let denied = mcp_call(&request, &session, &service.plaintext, 5, "tools/call",
            json!({"name":"delete_template_library_item","arguments":{"id":id}})).await;
        assert_eq!(denied["result"]["isError"], true);
        let deleted = mcp_call(&request, &session, &tenant_admin_key, 6, "tools/call",
            json!({"name":"delete_template_library_item","arguments":{"id":id}})).await;
        assert_eq!(tool_result_json(&deleted)["deleted"], true);
        let rest = request.get(&format!("/api/template-library/{id}")).add_header("Authorization", format!("Bearer {tenant_admin_key}")).await;
        rest.assert_status(StatusCode::NOT_FOUND);
        let rest = request.get(&format!("/api/template-library/{}",fork["id"].as_str().unwrap())).add_header("Authorization", format!("Bearer {tenant_admin_key}")).await;
        rest.assert_status_ok();
    }).await;
}
