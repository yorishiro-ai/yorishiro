//! The paid edition's MCP surface: this crate's own tools served alongside the base edition's.
//!
//! `ToolRouter<T>` ties a tool to the struct whose `impl` block declares it, so a crate outside
//! `yorishiro-server` cannot add entries to that server's router. This wraps the base server
//! instead, answering `tools/list` with both sets and routing `tools/call` by name. The base
//! edition's 23 tools keep their behaviour exactly, since the delegated call is the same one
//! the community binary makes.
//!
//! The four methods below are the whole of what the base server customises: `get_info`, and the
//! three `#[tool_handler]` generates for it. Every other [`ServerHandler`] method is the trait
//! default on both sides, so leaving them alone keeps the two editions identical rather than
//! silently dropping behaviour -- add a delegation here if the base server ever overrides one.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ListToolsResult, PaginatedRequestParams, ServerInfo,
    Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, tool_router};
use yorishiro_server::AppState;
use yorishiro_server::http::mcp::YorishiroMcpServer;

/// Serves the base edition's tools plus this crate's.
#[derive(Clone)]
pub struct HostedMcpServer {
    base: YorishiroMcpServer,
    tool_router: ToolRouter<Self>,
}

impl HostedMcpServer {
    pub fn new(state: AppState) -> Self {
        Self {
            base: YorishiroMcpServer::new(state),
            tool_router: Self::tool_router_hosted(),
        }
    }
}

/// This crate's own tools. Empty for now: the wrapper exists so adding one is a local change
/// here rather than a change to `build_app`'s contract, and an empty router leaves `tools/list`
/// answering exactly the base edition's 23.
#[tool_router(router = tool_router_hosted, vis = "pub(self)")]
impl HostedMcpServer {}

impl ServerHandler for HostedMcpServer {
    fn get_info(&self) -> ServerInfo {
        self.base.get_info()
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut listed = self.base.list_tools(request, context).await?;
        listed.tools.extend(self.tool_router.list_all());
        Ok(listed)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        // Ours first, so a name this crate defines wins over a base tool of the same name --
        // the same precedence `hosted_router` already has over the base router for REST paths.
        if self.tool_router.has_route(request.name.as_ref()) {
            let context = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
            return self.tool_router.call(context).await;
        }
        self.base.call_tool(request, context).await
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router
            .get(name)
            .cloned()
            .or_else(|| self.base.get_tool(name))
    }
}

#[cfg(test)]
#[path = "../../tests/http/mcp.rs"]
mod tests;
