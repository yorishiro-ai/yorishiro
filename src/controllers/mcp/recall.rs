use crate::controllers::mcp::{render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use super::YorishiroMcpServer;
use crate::models::recall::{self, DEFAULT_RECALL_DEPTH, DEFAULT_RECALL_LIMIT};

#[derive(Deserialize, JsonSchema)]
pub(crate) struct RecallContextArgs {
    pub entity_id: Uuid,
    /// Maximum number of relations/neighbors to include per hop (defaults to 20 if omitted).
    pub limit: Option<i64>,
    /// When true, neighbor entities include every field instead of only `x-embed` fields (defaults to false).
    pub full: Option<bool>,
    /// How many hops to traverse outward from the entity (defaults to 1, max 3).
    pub depth: Option<i64>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_recall)]
impl YorishiroMcpServer {
    #[tool(
        description = "Fetch an entity with its relations and connected neighbors up to `depth` hops away."
    )]
    pub async fn recall_entity_context(
        &self,
        Parameters(args): Parameters<RecallContextArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let query = recall::RecallQuery {
            limit: args.limit.unwrap_or(DEFAULT_RECALL_LIMIT),
            full: args.full.unwrap_or(false),
            depth: args.depth.unwrap_or(DEFAULT_RECALL_DEPTH),
        };
        render(
            recall::recall_context_for_actor(self.app_context(), actor, args.entity_id, query)
                .await,
        )
    }
}
