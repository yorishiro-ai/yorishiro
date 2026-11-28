use crate::controllers::mcp::{try_tool, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{YorishiroMcpServer, ok_json};
use crate::controllers::extractors::search_token_limiter;
use crate::models::search;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct SearchEntitiesArgs {
    /// Natural-language query text.
    /// Vectorized via the embedding provider and matched against entities' `x-embed` field by cosine distance.
    /// Entities whose embedding is not stored yet do not appear in results.
    pub query_text: String,
    pub entity_type: Option<String>,
    /// JSON containment filter applied to the nearest candidates, matched against entity data, e.g. `{"status": "active"}`.
    pub filter: Option<Value>,
    /// Upper bound on the number of results returned (defaults to 10 if omitted).
    pub limit: Option<i64>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_search)]
impl YorishiroMcpServer {
    #[tool(
        description = "Search entities by embedding similarity. Results are ranked by cosine distance."
    )]
    pub(crate) async fn search_entities(
        &self,
        Parameters(args): Parameters<SearchEntitiesArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth_ctx = verified!(self, parts);

        let default = search::SearchQuery::default();
        let query = search::SearchQuery {
            entity_type: args.entity_type,
            filter: args.filter,
            limit: args.limit.unwrap_or(default.limit),
        };

        let limiter = try_tool!(search_token_limiter(self.app_context()).map_err(|err| err.0));
        let hits = try_tool!(
            search::search_for_actor(
                self.app_context(),
                &auth_ctx,
                &args.query_text,
                query,
                &limiter,
            )
            .await
        );

        ok_json(hits)
    }
}
