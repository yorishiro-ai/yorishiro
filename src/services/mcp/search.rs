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
use crate::controllers::extractors::{db_handle, resolve_embedding_provider, search_token_limiter};
use crate::error::YorishiroError;
use crate::models::search;
use crate::services::auth::ApiKeyScope;
use crate::services::rate_limit::charge_search_tokens;

#[derive(Deserialize, JsonSchema)]
pub struct SearchEntitiesArgs {
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
    pub async fn search_entities(
        &self,
        Parameters(args): Parameters<SearchEntitiesArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth_ctx = match super::verify(&self.ctx, &parts, ApiKeyScope::Read).await? {
            VerifyOutcome::Verified(auth_ctx) => auth_ctx,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let default = search::SearchQuery::default();
        let query = search::SearchQuery {
            entity_type: args.entity_type,
            filter: args.filter,
            limit: args.limit.unwrap_or(default.limit),
        };

        let provider = match resolve_embedding_provider(&self.ctx, auth_ctx.workspace_id)
            .await
            .map_err(|err| err.0)
        {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        let limiter = match search_token_limiter(&self.ctx).map_err(|err| err.0) {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        // Charged before embedding, same as the REST adapter: the budget bounds embedding work, and this tool does exactly as much of it as `GET /api/search`.
        match charge_search_tokens(
            &limiter,
            provider.as_ref(),
            auth_ctx.workspace_id,
            &args.query_text,
        ) {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };

        // Embedding generation happens before acquiring a DB connection: don't hold a pool connection while waiting on the provider's HTTP round trip.
        let vector = match search::embed_query(provider.as_ref(), &args.query_text).await {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };

        // Vector search uses `content_entities.embedding` which does not exist on SQLite.
        if self.ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
            return Ok(err_to_tool_result(YorishiroError::BackendUnsupported {
                message: "vector search requires PostgreSQL; point DATABASE_URL at a PostgreSQL instance to use this feature".into(),
            }));
        }

        let workspace_id = auth_ctx.workspace_id;
        let db = match db_handle(&self.ctx).map_err(|err| err.0) {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        // A read-only transaction, same as `Authorized`'s: dropped without committing when this returns, which is a no-op since nothing was written.
        let txn = match db
            .tenant
            .begin_for_workspace(auth_ctx.tenant_id, workspace_id)
            .await
            .map_err(|err| crate::error::YorishiroError::Internal(err.into()))
        {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };

        let hits =
            match search::search_by_vector(&txn, workspace_id, vector, &args.query_text, query)
                .await
            {
                Ok(value) => value,
                Err(err) => return Ok(err_to_tool_result(err)),
            };
        ok_json(hits)
    }
}
