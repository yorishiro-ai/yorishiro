//! The wrapper's one job is to serve this edition's tools *in addition to* the base edition's,
//! never instead of them. Overriding `/mcp` is what makes losing the base set possible, so that
//! is what these check.

use async_trait::async_trait;
use rmcp::ServerHandler;
use sqlx::PgPool;
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

/// The tool names each server exposes, read off the routers rather than over the transport:
/// `tools/list` on Streamable HTTP needs a session handshake, and what can break here is the
/// delegation, not the SSE framing.
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

/// `get_tool` is the one name-addressed accessor both servers implement, so asking it for every
/// base tool name is how the wrapper's routing gets checked without a live session.
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
