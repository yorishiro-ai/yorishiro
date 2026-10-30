mod dashboard;
mod embedding;
mod entity_columns;
mod llm;
mod origin;
mod schema_forks;
mod worker_class;

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;

use crate::controllers::mcp::McpToolPolicy;
use crate::controllers::mcp::YorishiroMcpServer;
use crate::controllers::mcp::compose_tool_routers;

struct ManagementToolPolicy;

impl McpToolPolicy for ManagementToolPolicy {
    fn allows(&self, _ctx: &loco_rs::app::AppContext, _name: &str) -> bool {
        // These operations are ungated on both transports, per the edition policy.
        true
    }
}

/// The complete enterprise-only MCP tool set.
pub fn tool_router() -> ToolRouter<YorishiroMcpServer> {
    compose_tool_routers([
        YorishiroMcpServer::tool_router_dashboard(),
        YorishiroMcpServer::tool_router_origin(),
        YorishiroMcpServer::tool_router_entity_columns(),
        YorishiroMcpServer::tool_router_worker_class(),
        YorishiroMcpServer::tool_router_embedding(),
        YorishiroMcpServer::tool_router_llm(),
        YorishiroMcpServer::tool_router_schema_forks(),
    ])
}

pub(crate) fn tool_policy() -> Arc<dyn McpToolPolicy> {
    Arc::new(ManagementToolPolicy)
}
