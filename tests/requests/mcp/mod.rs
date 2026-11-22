mod api_keys;
mod audit_log;
mod authorize;
mod entities;
mod export;
mod import;
mod members;
mod schemas;
mod search;
mod template_library;
mod users;
mod whoami;

#[cfg(feature = "enterprise")]
use crate::requests::{
    boot_request,
    fixtures::{TenantArgs, TestTenant},
};
use axum::http::StatusCode;
use loco_rs::TestServer;
use serde_json::{Value, json};
#[cfg(feature = "enterprise")]
use yorishiro::App;
#[cfg(feature = "enterprise")]
use yorishiro::models::api_keys::ApiKeyScope;

#[cfg(feature = "enterprise")]
/// One provider kind's MCP tools: the secret never comes back, REST agrees with MCP, a read key cannot write, and an invalid input leaves the saved value untouched.
#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
pub(crate) async fn provider_tools_keep_secrets_private_and_match_rest(
    kind: &str,
    rest_path: &str,
    input: Value,
    secret: &str,
    shown: (&str, Value),
    invalid: (&str, Value),
) {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let tool = |action: &str| format!("{action}_{kind}_provider");
        let saved = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            2,
            "tools/call",
            json!({"name":tool("set"),"arguments":input}),
        )
        .await;
        assert_eq!(tool_result_json(&saved)["saved"], true);
        assert!(!saved.to_string().contains(secret));
        let read_key = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            Some(crate::requests::fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await),
            false,
        )
        .await
        .unwrap()
        .plaintext;
        let current = mcp_call(
            &request,
            &session,
            &read_key,
            3,
            "tools/call",
            json!({"name":tool("get"),"arguments":{}}),
        )
        .await;
        let metadata = tool_result_json(&current);
        assert!(metadata.get("api_key").is_none());
        assert!(!current.to_string().contains(secret));
        assert_eq!(metadata[shown.0], shown.1);
        let http_response = request
            .get(rest_path)
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        http_response.assert_status_ok();
        assert_eq!(metadata, http_response.json::<Value>());
        for (id, name, arguments) in [
            (4, tool("set"), input.clone()),
            (5, tool("reset"), json!({})),
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
        let mut bad = input;
        bad[invalid.0] = invalid.1;
        let rejected = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            6,
            "tools/call",
            json!({"name":tool("set"),"arguments":bad}),
        )
        .await;
        assert_eq!(rejected["result"]["isError"], true);
        let unchanged = mcp_call(
            &request,
            &session,
            &read_key,
            7,
            "tools/call",
            json!({"name":tool("get"),"arguments":{}}),
        )
        .await;
        assert_eq!(tool_result_json(&unchanged), metadata);
        let reset = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            8,
            "tools/call",
            json!({"name":tool("reset"),"arguments":{}}),
        )
        .await;
        assert_eq!(tool_result_json(&reset)["reset"], true);
        request
            .get(rest_path)
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await
            .assert_status(StatusCode::NOT_FOUND);
    })
    .await;
}

pub(crate) fn rpc_body(text: &str) -> Value {
    let payload = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .find(|line| !line.is_empty())
        .unwrap_or_else(|| text.trim());
    serde_json::from_str(payload).unwrap_or_else(|err| panic!("invalid MCP response {err}: {text}"))
}

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
pub(crate) async fn initialize(request: &TestServer) -> String {
    let response = request
        .post("/mcp")
        .add_header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "yorishiro-test", "version": "1.0" }
            }
        }))
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::OK,
        "response: {}",
        response.text()
    );
    let _ = rpc_body(&response.text());
    let session = response
        .headers()
        .get("mcp-session-id")
        .expect("MCP session id")
        .to_str()
        .expect("MCP session id is text")
        .to_owned();

    let response = request
        .post("/mcp")
        .add_header("Accept", "application/json, text/event-stream")
        .add_header("MCP-Session-Id", session.clone())
        .add_header("MCP-Protocol-Version", "2025-03-26")
        .json(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::ACCEPTED,
        "response: {}",
        response.text()
    );
    session
}

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
pub(crate) async fn mcp_call(
    request: &TestServer,
    session: &str,
    api_key: &str,
    id: i64,
    method: &str,
    params: Value,
) -> Value {
    let response = request
        .post("/mcp")
        .add_header("Accept", "application/json, text/event-stream")
        .add_header("MCP-Session-Id", session)
        .add_header("MCP-Protocol-Version", "2025-03-26")
        .add_header("Authorization", format!("Bearer {api_key}"))
        .json(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::OK,
        "response: {}",
        response.text()
    );
    rpc_body(&response.text())
}

pub(crate) fn tool_inventory(response: &Value) -> &[Value] {
    response["result"]["tools"]
        .as_array()
        .expect("tools/list result")
}

pub(crate) fn tool_result_json(response: &Value) -> Value {
    assert_ne!(
        response["result"]["isError"], true,
        "expected a successful tool result: {response}"
    );
    serde_json::from_str(
        response["result"]["content"][0]["text"]
            .as_str()
            .expect("tool result text"),
    )
    .expect("JSON tool result")
}
