use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use utoipa::IntoParams;
use yorishiro_core::repositories::search::{self, SearchHit};
use yorishiro_core::{ResultExt, YorishiroError};

use crate::error::ApiError;
use crate::http::middleware::auth::{ReadScope, Verified};
use crate::state::AppState;

#[derive(Debug, Deserialize, IntoParams)]
pub struct SearchEntitiesParams {
    pub query_text: String,
    pub entity_type: Option<String>,
    /// JSON-encoded containment filter, e.g. `{"status":"active"}`.
    pub filter: Option<String>,
    pub limit: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/search",
    params(SearchEntitiesParams),
    responses(
        (status = 200, description = "Vector similarity search results for a natural-language query", body = Vec<SearchHit>),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
    ),
    tag = "search",
)]
pub async fn search_entities(
    State(state): State<AppState>,
    // `Verified`, not `Authorized`: no connection is acquired here, since one
    // isn't needed until after the slow embedding call below.
    verified: Verified<ReadScope>,
    Query(params): Query<SearchEntitiesParams>,
) -> Result<Json<Vec<SearchHit>>, ApiError> {
    let default = search::SearchQuery::default();
    let query = search::SearchQuery {
        entity_type: params.entity_type,
        filter: crate::http::controllers::parse_filter_param(params.filter)?,
        limit: params.limit.unwrap_or(default.limit),
    };

    // Charged before embedding, since embedding is the work the budget protects. Counting is
    // cheap here (a query is short), which is why search is metered in tokens while writes
    // stay on request counts.
    let tokens = state.embedding_provider.count_tokens(&params.query_text);
    if !state
        .search_token_limiter
        .allow_cost(&verified.ctx.workspace_id.to_string(), tokens)
    {
        tracing::warn!(
            workspace_id = %verified.ctx.workspace_id,
            tokens,
            "search token budget exhausted"
        );
        return Err(YorishiroError::ValidationFailed {
            message: "this workspace has spent its search token budget for the minute".to_string(),
            details: vec![],
            hint: "retry shortly, or raise YORISHIRO_SEARCH_TOKENS_PER_MINUTE".to_string(),
        }
        .into());
    }

    // Embedding generation happens before acquiring a DB connection. The
    // LocalOnnx provider serializes inference within the process, so holding a
    // connection while waiting would let pool exhaustion spill over to other
    // endpoints too.
    let vector = search::embed_query(state.embedding_provider.as_ref(), &params.query_text).await?;

    let workspace_id = verified.ctx.workspace_id;
    let mut conn = state
        .tenant_db
        .acquire_for_workspace(verified.ctx.tenant_id, workspace_id)
        .await
        .internal()?;
    let hits = search::search_by_vector(&mut conn, workspace_id, vector, &params.query_text, query)
        .await?;
    Ok(Json(hits))
}

#[cfg(test)]
#[path = "../../../tests/http/controllers/search.rs"]
mod tests;
