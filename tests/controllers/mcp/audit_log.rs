use serde_json::{Value, json};
use yorishiro::App;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant, issue_access_key};

use super::*;

#[tokio::test]
async fn audit_tool_uses_iam_and_matches_rest() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let administrator =
            crate::requests::fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let audit_key = issue_access_key(&ctx, tenant.workspace_id, administrator).await;
        let listed = mcp_call(
            &request,
            &session,
            &audit_key,
            3,
            "tools/call",
            json!({"name":"list_workspace_audit_events","arguments":{"page":1,"page_size":10}}),
        )
        .await;
        let rest = request
            .get("/api/audit-log?page=1&page_size=10")
            .add_header("Authorization", format!("Bearer {audit_key}"))
            .await;
        rest.assert_status_ok();
        assert_eq!(tool_result_json(&listed), rest.json::<Value>());
        let invalid = mcp_call(
            &request,
            &session,
            &audit_key,
            4,
            "tools/call",
            json!({"name":"list_workspace_audit_events","arguments":{"page":u64::MAX}}),
        )
        .await;
        assert_eq!(invalid["result"]["isError"], true);
    })
    .await;
}
