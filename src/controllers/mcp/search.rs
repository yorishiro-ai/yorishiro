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

use super::{VerifyOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::controllers::extractors::search_token_limiter;
use crate::controllers::middleware::rate_limit::charge_search_tokens;
use crate::models::api_keys::ApiKeyScope;
use crate::models::search;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct SearchEntitiesArgs {
    /// Natural-language query text.
    /// Vectorized via the embedding provider and matched against entities' `x-embed` field by cosine distance.
    /// Also used, as-is, for an auxiliary pg_trgm fuzzy text match against entities that have no embedding.
    pub query_text: String,
    pub entity_type: Option<String>,
    /// JSONB containment filter matched against entity data, e.g. `{"status": "active"}`.
    pub filter: Option<Value>,
    /// Upper bound on the number of results returned (defaults to 10 if omitted).
    pub limit: Option<i64>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_search)]
impl YorishiroMcpServer {
    #[tool(
        description = "Vector similarity search over entities using a natural-language query (requires read scope)"
    )]
    pub(crate) async fn search_entities(
        &self,
        Parameters(args): Parameters<SearchEntitiesArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth_ctx = match super::verify(self.app_context(), &parts, ApiKeyScope::Read).await? {
            VerifyOutcome::Verified(auth_ctx) => auth_ctx,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let default = search::SearchQuery::default();
        let query = search::SearchQuery {
            entity_type: args.entity_type,
            filter: args.filter,
            limit: args.limit.unwrap_or(default.limit),
        };

        let workspace_id = auth_ctx.workspace_id;
        let limiter = match search_token_limiter(self.app_context()).map_err(|err| err.0) {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        // Charged before embedding, same as the REST adapter: the budget bounds embedding work, and this tool does exactly as much of it as `GET /api/search`.
        if let Err(err) = charge_search_tokens(&limiter, workspace_id, &args.query_text) {
            return Ok(err_to_tool_result(err));
        }

        // The server holds no model: a worker embeds the query, and no DB connection is held while waiting for it.
        let vector = match crate::workers::query_embedding::embed_query(
            self.app_context(),
            auth_ctx.tenant_id,
            workspace_id,
            &args.query_text,
        )
        .await
        {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };

        // Resolved on the identity pool: the tenant-scoped role cannot read `tenant_tenants`, which the embedding chain joins.
        let embed_table =
            match search::resolve_query_table(&self.app_context().db, workspace_id, vector.len())
                .await
            {
                Ok(value) => value,
                Err(err) => return Ok(err_to_tool_result(err)),
            };

        // A read-only transaction, same as `Authorized`'s: dropped without committing when this returns, which is a no-op since nothing was written.
        let txn =
            match crate::db::begin_workspace(self.app_context(), auth_ctx.tenant_id, workspace_id)
                .await
            {
                Ok(value) => value,
                Err(err) => return Ok(err_to_tool_result(err)),
            };

        let hits = match search::search_in_table(
            &txn,
            workspace_id,
            vector,
            &args.query_text,
            query,
            embed_table,
        )
        .await
        {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        ok_json(hits)
    }
}
