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

use super::{AuthzOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::models::recall::{self, DEFAULT_RECALL_DEPTH, DEFAULT_RECALL_LIMIT};
use crate::services::auth::ApiKeyScope;

#[derive(Deserialize, JsonSchema)]
pub struct RecallContextArgs {
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
        description = "Fetch an entity's full body together with its relations and connected neighbors, up to `depth` hops away, in one call (requires read scope)"
    )]
    pub async fn recall_context(
        &self,
        Parameters(args): Parameters<RecallContextArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(&self.ctx, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let workspace_id = authorized.ctx.workspace_id;
        let query = recall::RecallQuery {
            limit: args.limit.unwrap_or(DEFAULT_RECALL_LIMIT),
            full: args.full.unwrap_or(false),
            depth: args.depth.unwrap_or(DEFAULT_RECALL_DEPTH),
        };
        let context =
            match recall::recall_context(authorized.txn(), workspace_id, args.entity_id, query)
                .await
            {
                Ok(value) => value,
                Err(err) => return Ok(err_to_tool_result(err)),
            };
        ok_json(context)
    }
}
