use serde_json::{Value, json};
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant, issue_api_key};

use super::*;

#[tokio::test]
async fn audit_tool_preserves_independent_grant_and_rest_parity() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let denied = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            2,
            "tools/call",
            json!({"name":"list_audit_log","arguments":{}}),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true);
        let audit_key = issue_api_key(
            &ctx,
            tenant.workspace_id,
            tenant.owner_id,
            ApiKeyScope::Read,
            true,
        )
        .await;
        let listed = mcp_call(
            &request,
            &session,
            &audit_key,
            3,
            "tools/call",
            json!({"name":"list_audit_log","arguments":{"page":1,"page_size":10}}),
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
            json!({"name":"list_audit_log","arguments":{"page":u64::MAX}}),
        )
        .await;
        assert_eq!(invalid["result"]["isError"], true);
        for (key, expected) in [(&tenant.api_key, false), (&audit_key, true)] {
            let capabilities = mcp_call(
                &request,
                &session,
                key,
                5,
                "tools/call",
                json!({"name":"capabilities","arguments":{}}),
            )
            .await;
            let capabilities = tool_result_json(&capabilities);
            let audit = capabilities["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == "list_audit_log")
                .unwrap();
            assert_eq!(audit["required_grants"], json!(["audit"]));
            assert_eq!(audit["grants_satisfied"], expected);
        }
    })
    .await;
}
