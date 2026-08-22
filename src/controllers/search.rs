use axum::Json;
use axum::extract::{Query, State};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::Deserialize;

use crate::controllers::ApiError;
use crate::controllers::extractors::{ReadScope, Verified, db_handle, embedding_provider};
use crate::error::YorishiroError;
use crate::models::search::{self, SearchHit};

#[derive(Debug, Deserialize)]
pub struct SearchEntitiesParams {
    pub query_text: String,
    pub entity_type: Option<String>,
    /// JSON-encoded containment filter, e.g. `{"status":"active"}`.
    pub filter: Option<String>,
    pub limit: Option<i64>,
}

pub async fn search_entities(
    State(ctx): State<AppContext>,
    // `Verified`, not `Authorized`: no connection is acquired here, since one isn't needed until
    // after the slow embedding call below.
    verified: Verified<ReadScope>,
    Query(params): Query<SearchEntitiesParams>,
) -> Result<Json<Vec<SearchHit>>, ApiError> {
    let default = search::SearchQuery::default();
    let query = search::SearchQuery {
        entity_type: params.entity_type,
        filter: super::parse_filter_param(params.filter)?,
        limit: params.limit.unwrap_or(default.limit),
    };

    let provider = embedding_provider(&ctx)?;

    // Embedding generation happens before acquiring a DB connection: don't hold a pool
    // connection while waiting on the provider's HTTP round trip.
    let vector = search::embed_query(provider.as_ref(), &params.query_text).await?;

    let workspace_id = verified.ctx.workspace_id;
    let db = db_handle(&ctx)?;
    // A read-only transaction: dropped without committing when this returns, a no-op since
    // nothing was written.
    let txn = db
        .tenant
        .begin_for_workspace(verified.ctx.tenant_id, workspace_id)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    let hits =
        search::search_by_vector(&txn, workspace_id, vector, &params.query_text, query).await?;
    Ok(Json(hits))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/search")
        .add("/", axum::routing::get(search_entities))
}
