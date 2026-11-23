use axum::http::StatusCode;
use serde_json::{Value, json};
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant, issue_tenant_key};

use super::{initialize, mcp_call, tool_result_json};

#[tokio::test]
async fn tenant_overview_tool_matches_rest_and_rejects_service_credentials() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let root = issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let session = initialize(&request).await;
        let arguments = json!({"name":"get_tenant_overview","arguments":{}});
        let overview = mcp_call(
            &request,
            &session,
            &root,
            2,
            "tools/call",
            arguments.clone(),
        )
        .await;
        let rest = request
            .get("/api/tenant/overview")
            .add_header("Authorization", format!("Bearer {root}"))
            .await;
        rest.assert_status_ok();
        assert_eq!(tool_result_json(&overview), rest.json::<Value>());
        let service = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            None,
            false,
        )
        .await
        .unwrap();
        let denied = mcp_call(
            &request,
            &session,
            &service.plaintext,
            3,
            "tools/call",
            arguments,
        )
        .await;
        assert_eq!(denied["result"]["isError"], true);
        request
            .get("/api/tenant/overview")
            .add_header("Authorization", format!("Bearer {}", service.plaintext))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    })
    .await;
}
