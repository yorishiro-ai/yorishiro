//! The enterprise MCP tool set and the policy that classifies and gates its tools.
//! Each submodule defines the tools of one enterprise feature, and `tool_router` composes them for the overlay.

mod embedding;
mod entity_columns;
mod inference;
mod llm;
mod marketplace;
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
    fn accepts_workspace_selection(&self, name: &str) -> bool {
        crate::controllers::mcp::policy::workspace_selection_tool(name)
            || matches!(
                name,
                "confirm_inference_proposals"
                    | "create_schema_fork"
                    | "delete_workspace_embedding_provider"
                    | "delete_workspace_llm_provider"
                    | "delete_schema_fork"
                    | "discard_inference_proposals"
                    | "get_workspace_embedding_provider"
                    | "get_inference_confirmation"
                    | "get_inference_job"
                    | "get_workspace_llm_provider"
                    | "get_schema_fork"
                    | "get_workspace_worker_class"
                    | "create_inference_job"
                    | "list_workspace_entity_columns"
                    | "list_inference_proposals"
                    | "list_schema_forks"
                    | "list_schema_upstream_changes"
                    | "apply_schema_merge"
                    | "preview_schema_merge"
                    | "delete_workspace_entity_columns"
                    | "delete_workspace_worker_class"
                    | "update_workspace_embedding_provider"
                    | "update_workspace_entity_columns"
                    | "update_workspace_llm_provider"
                    | "update_workspace_worker_class"
                    | "update_schema_fork"
            )
    }

    fn allows(&self, ctx: &loco_rs::app::AppContext, name: &str) -> bool {
        if marketplace::TOOL_NAMES.contains(&name) || inference::TOOL_NAMES.contains(&name) {
            return crate::edition::ee::controllers::middleware::edition::is_active(ctx);
        }
        true
    }
}

/// The complete enterprise-only MCP tool set.
#[must_use]
pub fn tool_router() -> ToolRouter<YorishiroMcpServer> {
    compose_tool_routers([
        YorishiroMcpServer::tool_router_origin(),
        YorishiroMcpServer::tool_router_entity_columns(),
        YorishiroMcpServer::tool_router_worker_class(),
        YorishiroMcpServer::tool_router_embedding(),
        YorishiroMcpServer::tool_router_llm(),
        YorishiroMcpServer::tool_router_schema_forks(),
        YorishiroMcpServer::tool_router_marketplace(),
        YorishiroMcpServer::tool_router_inference(),
    ])
}

pub(crate) fn tool_policy() -> Arc<dyn McpToolPolicy> {
    Arc::new(ManagementToolPolicy)
}
