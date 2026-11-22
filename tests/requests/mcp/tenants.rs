use serde_json::{Value, json};
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant, issue_api_key};

use super::*;

#[tokio::test]
async fn tenant_metadata_tools_match_rest_and_keep_updates_atomic() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_admin_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let session = initialize(&request).await;
        let updated = mcp_call(&request, &session, &tenant_admin_key, 2, "tools/call", json!({"name":"update_tenant","arguments":{"name":" managed tenant ","replace_max_workspaces":true,"max_workspaces":0}})).await;
        let record = tool_result_json(&updated);
        assert_eq!(record["name"], "managed tenant");
        assert_eq!(record["max_workspaces"], 0);
        let rest = request.get("/api/tenants/current").add_header("Authorization", format!("Bearer {tenant_admin_key}")).await;
        rest.assert_status_ok();
        assert_eq!(record, rest.json::<Value>());
        let blocked = mcp_call(&request, &session, &tenant_admin_key, 3, "tools/call", json!({"name":"create_workspace","arguments":{"name":"blocked"}})).await;
        assert_eq!(blocked["result"]["isError"], true);
        for arguments in [json!({"name":"rollback","replace_max_workspaces":true,"max_workspaces":-1}), json!({"name":"rollback","max_workspaces":5}), json!({"name":" "})] {
            let invalid = mcp_call(&request, &session, &tenant_admin_key, 4, "tools/call", json!({"name":"update_tenant","arguments":arguments})).await;
            assert_eq!(invalid["result"]["isError"], true);
        }
        let detail = mcp_call(&request, &session, &tenant_admin_key, 5, "tools/call", json!({"name":"get_tenant","arguments":{}})).await;
        assert_eq!(tool_result_json(&detail), record);
        let cleared = request.put("/api/tenants/current").add_header("Authorization", format!("Bearer {tenant_admin_key}")).json(&json!({"replace_max_workspaces":true,"max_workspaces":null})).await;
        cleared.assert_status_ok();
        assert!(cleared.json::<Value>()["max_workspaces"].is_null());
        let user = yorishiro::models::user_users::create_user(&ctx.db, &crate::requests::fixtures::unique_email("tenant-viewer"), "test-password", None).await.unwrap();
        yorishiro::models::tenant_memberships::add_member(&ctx.db, tenant.tenant_id, user.id).await.unwrap();
        crate::requests::fixtures::grant_workspace(&ctx, tenant.workspace_id, user.id, yorishiro::models::workspace_memberships::WorkspaceRole::Viewer).await;
        let viewer = issue_api_key(&ctx, tenant.workspace_id, user.id, ApiKeyScope::Read, false).await;
        for (name, args) in [("get_tenant", json!({})), ("update_tenant", json!({"name":"forbidden"}))] {
            let denied = mcp_call(&request, &session, &viewer, 6, "tools/call", json!({"name":name,"arguments":args})).await;
            assert_eq!(denied["result"]["isError"], true);
        }
    }).await;
}
