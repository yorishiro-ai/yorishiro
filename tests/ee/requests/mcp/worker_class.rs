use axum::http::StatusCode;
use serde_json::{Value, json};
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

use super::*;

#[tokio::test]
async fn worker_class_tools_match_rest_and_enforce_schema_scope() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let saved = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            2,
            "tools/call",
            json!({"name":"set_worker_class","arguments":{"worker_class":"shared"}}),
        )
        .await;
        assert_eq!(tool_result_json(&saved)["saved"], true);
        let current = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            3,
            "tools/call",
            json!({"name":"get_worker_class","arguments":{}}),
        )
        .await;
        let rest = request
            .get("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        rest.assert_status_ok();
        assert_eq!(tool_result_json(&current), rest.json::<Value>());
        assert_eq!(tool_result_json(&current)["worker_class"], "shared");
        let read_key = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            None,
            false,
        )
        .await
        .unwrap()
        .plaintext;
        for (id, name, arguments) in [
            (4, "set_worker_class", json!({"worker_class":"shared"})),
            (5, "reset_worker_class", json!({})),
        ] {
            let denied = mcp_call(
                &request,
                &session,
                &read_key,
                id,
                "tools/call",
                json!({"name":name,"arguments":arguments}),
            )
            .await;
            assert_eq!(denied["result"]["isError"], true);
        }
        let reset = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            6,
            "tools/call",
            json!({"name":"reset_worker_class","arguments":{}}),
        )
        .await;
        assert_eq!(tool_result_json(&reset)["reset"], true);
        request
            .get("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await
            .assert_status(StatusCode::NOT_FOUND);
        let absent = mcp_call(
            &request,
            &session,
            &read_key,
            7,
            "tools/call",
            json!({"name":"get_worker_class","arguments":{}}),
        )
        .await;
        assert_eq!(absent["result"]["isError"], true);
    })
    .await;
}
