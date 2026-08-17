//! The wrapper's one job is to serve this edition's tools *in addition to* the base edition's, never instead of them.
//! Overriding `/mcp` is what makes losing the base set possible, so that is what these check.

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use rmcp::ServerHandler;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::StreamableHttpService;
use sqlx::PgPool;
use tower::ServiceExt;
use yorishiro_core::YorishiroError;
use yorishiro_core::services::embedding::EmbeddingProvider;
use yorishiro_server::AppState;
use yorishiro_server::http::mcp::YorishiroMcpServer;

use crate::http::mcp::HostedMcpServer;

/// Never called: `get_tool`/the tool router read the registry, not the provider.
struct UnusedEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for UnusedEmbeddingProvider {
    fn dimensions(&self) -> usize {
        768
    }

    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        unreachable!("listing tools does not embed")
    }
}

/// The tool names each server exposes, read through `get_tool` rather than over the transport.
/// The two transport tests below cover `tools/list` and `tools/call`; this pair isolates the name lookup, so a failure says which of the three delegations broke.
fn tool_names(pool: PgPool) -> (Vec<String>, Vec<String>) {
    let state = AppState::new(
        yorishiro_core::db::TenantDb::new(pool.clone()),
        pool,
        std::sync::Arc::new(UnusedEmbeddingProvider),
    );
    let base = YorishiroMcpServer::new(state.clone());
    let hosted = HostedMcpServer::new(state);
    (base_names(&base), hosted_names(&hosted))
}

/// `get_tool` is the one name-addressed accessor both servers implement, so asking it for every base tool name is how the wrapper's routing gets checked without a live session.
fn base_names(server: &YorishiroMcpServer) -> Vec<String> {
    KNOWN_BASE_TOOLS
        .iter()
        .filter(|name| server.get_tool(name).is_some())
        .map(|name| (*name).to_owned())
        .collect()
}

fn hosted_names(server: &HostedMcpServer) -> Vec<String> {
    KNOWN_BASE_TOOLS
        .iter()
        .filter(|name| server.get_tool(name).is_some())
        .map(|name| (*name).to_owned())
        .collect()
}

/// The base edition's tools, as its own suite asserts them (`expected 23 registered tools`).
/// Listed rather than counted so a rename shows up here as a missing name.
const KNOWN_BASE_TOOLS: &[&str] = &[
    "create_entity",
    "get_entity",
    "update_entity",
    "delete_entity",
    "list_entities",
    "get_entity_drift",
    "get_entity_type_json_schema",
    "import_jsonl",
    "recall_context",
    "create_relation",
    "get_relation",
    "delete_relation",
    "list_relations",
    "set_relation_status",
    "search_entities",
    "create_schema",
    "get_active_schema",
    "get_schema_by_id",
    "list_schemas",
    "list_templates",
    "migration_dry_run",
    "list_template_library",
    "get_template_library_item",
];

#[sqlx::test(migrations = "../../../migrations")]
async fn the_wrapper_serves_every_base_tool(pool: PgPool) {
    let (base, hosted) = tool_names(pool);

    assert_eq!(
        base.len(),
        KNOWN_BASE_TOOLS.len(),
        "the base edition no longer serves all of {KNOWN_BASE_TOOLS:?}; it served {base:?}"
    );
    for name in &base {
        assert!(
            hosted.contains(name),
            "`{name}` is served by the base edition but not through the wrapper; \
             overriding /mcp dropped it"
        );
    }
}

#[sqlx::test(migrations = "../../../migrations")]
async fn an_unknown_tool_is_unknown_to_both(pool: PgPool) {
    let state = AppState::new(
        yorishiro_core::db::TenantDb::new(pool.clone()),
        pool,
        std::sync::Arc::new(UnusedEmbeddingProvider),
    );
    let hosted = HostedMcpServer::new(state);

    assert!(
        hosted.get_tool("no_such_tool").is_none(),
        "the wrapper invented a tool neither edition defines"
    );
}

/// Mounts the wrapper exactly as `main` does, so `list_tools` and `call_tool` are reached through the transport rather than called directly.
/// `RequestContext` needs a `Peer`, whose constructor is `pub(crate)` in rmcp, so the HTTP door is the only way in from here.
/// It is also the one that ships.
fn hosted_mcp_router(pool: PgPool) -> Router {
    let state = AppState::new(
        yorishiro_core::db::TenantDb::new(pool.clone()),
        pool,
        std::sync::Arc::new(UnusedEmbeddingProvider),
    );
    let service = StreamableHttpService::new(
        move || Ok(HostedMcpServer::new(state.clone())),
        LocalSessionManager::default().into(),
        Default::default(),
    );
    Router::new().nest_service("/mcp", service)
}

async fn mcp_post(
    app: &Router,
    session: Option<&str>,
    body: serde_json::Value,
) -> (StatusCode, String) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(session) = session {
        builder = builder.header("mcp-session-id", session);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let session_id = response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_default();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    (
        status,
        if session_id.is_empty() {
            body
        } else {
            session_id
        },
    )
}

/// The last `data:` line, which is the response to the request just sent.
fn sse_json(body: &str) -> serde_json::Value {
    body.split("\n\n")
        .filter_map(|event| event.lines().find_map(|line| line.strip_prefix("data: ")))
        .filter_map(|data| serde_json::from_str::<serde_json::Value>(data).ok())
        .last()
        .unwrap_or_else(|| panic!("no `data:` line in SSE body: {body:?}"))
}

async fn handshake(app: &Router) -> String {
    let (status, session) = mcp_post(
        app,
        None,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "yorishiro-hosted-test", "version": "0.0.0" },
            },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "initialize failed");
    assert!(!session.is_empty(), "no mcp-session-id returned");
    mcp_post(
        app,
        Some(&session),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    session
}

#[sqlx::test(migrations = "../../../migrations")]
async fn tools_list_over_the_transport_carries_the_base_tools(pool: PgPool) {
    let app = hosted_mcp_router(pool);
    let session = handshake(&app).await;

    let (status, body) = mcp_post(
        &app,
        Some(&session),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let json = sse_json(&body);
    let listed: Vec<String> = json["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list returned no array: {json}"))
        .iter()
        .map(|tool| {
            tool["name"]
                .as_str()
                .expect("tool without a name")
                .to_owned()
        })
        .collect();

    for name in KNOWN_BASE_TOOLS {
        assert!(
            listed.iter().any(|listed| listed == name),
            "`{name}` is missing from the wrapper's tools/list: {listed:?}"
        );
    }
}

#[sqlx::test(migrations = "../../../migrations")]
async fn tools_call_falls_through_to_the_base_server(pool: PgPool) {
    let app = hosted_mcp_router(pool);
    let session = handshake(&app).await;

    // No Authorization header, so the base tool refuses.
    // That refusal is the point: reaching it at all proves `call_tool` delegated, since this crate's router has no `list_schemas`.
    let (status, body) = mcp_post(
        &app,
        Some(&session),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "list_schemas", "arguments": {} },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let json = sse_json(&body);
    assert!(
        json.get("error").is_some() || json["result"]["isError"] == serde_json::json!(true),
        "expected the base tool's own refusal, got {json}"
    );
    let rendered = json.to_string();
    assert!(
        !rendered.contains("method not found") && !rendered.contains("-32601"),
        "the wrapper answered `unknown tool` instead of delegating: {json}"
    );
}
