mod origin;

use std::collections::HashSet;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;

use crate::controllers::mcp::McpToolPolicy;
use crate::controllers::mcp::YorishiroMcpServer;
use crate::controllers::mcp::compose_tool_routers;

struct LicenceToolPolicy {
    enterprise_tools: HashSet<String>,
}

impl McpToolPolicy for LicenceToolPolicy {
    fn allows(&self, ctx: &loco_rs::app::AppContext, name: &str) -> bool {
        !self.enterprise_tools.contains(name)
            || crate::edition::ee::controllers::middleware::edition::is_active(ctx)
    }
}

/// The complete enterprise-only MCP tool set.
pub fn tool_router() -> ToolRouter<YorishiroMcpServer> {
    compose_tool_routers([YorishiroMcpServer::tool_router_origin()])
}

pub(crate) fn tool_policy() -> Arc<dyn McpToolPolicy> {
    Arc::new(LicenceToolPolicy {
        enterprise_tools: tool_router().map.keys().map(ToString::to_string).collect(),
    })
}
