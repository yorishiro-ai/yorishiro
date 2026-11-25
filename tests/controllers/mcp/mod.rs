use std::collections::HashSet;
use std::sync::Arc;

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
use rmcp::handler::server::router::tool::{ToolRoute, ToolRouter};
use rmcp::model::{CallToolResult, Tool};
use serde_json::{Value, json};
#[cfg(feature = "enterprise")]
use yorishiro::App;
use yorishiro::controllers::mcp::{community_tool_router, compose_tool_routers};

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
            Some(crate::requests::fixtures::create_iam_reader(&ctx, tenant.tenant_id).await),
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

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
pub(crate) async fn call_tool(
    request: &TestServer,
    session: &str,
    key: &str,
    tool: &str,
    arguments: Value,
) -> Value {
    mcp_call(
        request,
        session,
        key,
        2,
        "tools/call",
        json!({"name": tool, "arguments": arguments}),
    )
    .await
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

mod input_parity_tests {
    use serde_json::json;
    use uuid::Uuid;
    use yorishiro::controllers::mcp::*;
    use yorishiro::dtos::{entities as rest_entities, relations as rest_relations};
    use yorishiro::models::{entity_entities, entity_relations};

    #[test]
    fn entity_list_rest_and_mcp_adapters_build_equivalent_queries() {
        let rest_query: entity_entities::ListEntitiesQuery = rest_entities::ListEntitiesParams {
            entity_type: Some("note".into()),
            filter: Some(r#"{"title":"hello"}"#.into()),
            schema_version: Some(2),
            page: yorishiro::dtos::common::PageParams(loco_rs::model::query::PaginationQuery {
                page: 3,
                page_size: 7,
            }),
        }
        .try_into()
        .expect("REST entity query");
        let mcp_query: entity_entities::ListEntitiesQuery = entities::ListEntitiesArgs {
            entity_type: Some("note".into()),
            filter: Some(json!({"title": "hello"})),
            schema_version: Some(2),
            limit: Some(7),
            offset: Some(14),
        }
        .try_into()
        .expect("MCP entity query");

        assert_eq!(rest_query.entity_type, mcp_query.entity_type);
        assert_eq!(rest_query.filter, mcp_query.filter);
        assert_eq!(rest_query.schema_version, mcp_query.schema_version);
        assert_eq!(rest_query.page.limit(), mcp_query.page.limit());
        assert_eq!(rest_query.page.offset(), mcp_query.page.offset());
    }

    #[test]
    fn relation_list_rest_and_mcp_adapters_build_equivalent_queries() {
        let source_id = Uuid::now_v7();
        let target_id = Uuid::now_v7();
        let rest_query: entity_relations::ListRelationsQuery =
            rest_relations::ListRelationsParams {
                source_id: Some(source_id),
                target_id: Some(target_id),
                relation_type: Some("knows".into()),
                status: Some(entity_relations::RelationStatus::Deprecated),
                page: yorishiro::dtos::common::PageParams(loco_rs::model::query::PaginationQuery {
                    page: 2,
                    page_size: 5,
                }),
            }
            .try_into()
            .expect("REST relation query");
        let mcp_query: entity_relations::ListRelationsQuery = relations::ListRelationsArgs {
            source_id: Some(source_id),
            target_id: Some(target_id),
            relation_type: Some("knows".into()),
            status: Some("deprecated".into()),
            limit: Some(5),
            offset: Some(5),
        }
        .try_into()
        .expect("MCP relation query");

        assert_eq!(rest_query.source_id, mcp_query.source_id);
        assert_eq!(rest_query.target_id, mcp_query.target_id);
        assert_eq!(rest_query.relation_type, mcp_query.relation_type);
        assert_eq!(rest_query.status, mcp_query.status);
        assert_eq!(rest_query.page.limit(), mcp_query.page.limit());
        assert_eq!(rest_query.page.offset(), mcp_query.page.offset());
    }
}

pub(crate) fn assert_inventory_contract(label: &str, tools: &[Tool]) {
    let names: Vec<_> = tools.iter().map(|tool| tool.name.to_string()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(
        names, sorted,
        "{label} MCP tool names are not sorted: {names:?}"
    );
    let unique: HashSet<_> = names.iter().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "duplicate {label} MCP tool names: {names:?}"
    );
    for tool in tools {
        assert!(
            tool.description
                .as_deref()
                .is_some_and(|description| !description.trim().is_empty()),
            "{label} tool {} has no description",
            tool.name
        );
        let schema = tool.schema_as_json_value();
        assert_eq!(
            schema["type"], "object",
            "invalid {label} schema for {}",
            tool.name
        );
        assert!(
            jsonschema::meta::options().is_valid(&schema),
            "invalid {label} input schema for {}: {schema}",
            tool.name
        );
    }
}

#[test]
fn community_inventory_contract_is_valid() {
    let community = community_tool_router().list_all();
    assert_inventory_contract("community", &community);
}

#[test]
fn composition_rejects_duplicate_registered_names_even_when_disabled() {
    fn router(name: &'static str) -> ToolRouter<yorishiro::controllers::mcp::YorishiroMcpServer> {
        ToolRouter::new().with_route(ToolRoute::new_dyn(
            Tool::new(name, "test tool", Arc::new(serde_json::Map::default())),
            |_context| Box::pin(async { Ok(CallToolResult::default().into()) }),
        ))
    }

    let disabled = router("duplicate").with_disabled("duplicate");
    assert_eq!(disabled.list_all(), [] as [Tool; 0]);
    assert!(disabled.map.contains_key("duplicate"));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compose_tool_routers([router("duplicate"), disabled]);
    }));
    assert!(
        result.is_err(),
        "duplicate registered names must be rejected"
    );
}
