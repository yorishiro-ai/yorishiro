use axum::Json;
use axum::extract::{Query, State};
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::extractors::{ReadScope, Verified, search_token_limiter};
use crate::controllers::openapi::Routes;
use crate::dtos::search::SearchEntitiesParams;
use crate::models::search::{self, SearchHit};

pub(crate) async fn search_entities(
    State(ctx): State<AppContext>,
    // `Verified`, not `Authorized`: no connection is acquired here until after the slow embedding call below.
    verified: Verified<ReadScope>,
    Query(params): Query<SearchEntitiesParams>,
) -> Result<Json<Vec<SearchHit>>, ApiError> {
    let default = search::SearchQuery::default();
    let query = search::SearchQuery {
        entity_type: params.entity_type,
        filter: crate::dtos::common::parse_filter_param(params.filter)?,
        limit: params.limit.unwrap_or(default.limit),
    };

    let limiter = search_token_limiter(&ctx)?;
    let hits =
        search::search_for_actor(&ctx, &verified.ctx, &params.query_text, query, &limiter).await?;
    Ok(Json(hits))
}

pub fn routes() -> Routes {
    Routes::new().tag("community").prefix("api/search").add(
        "/",
        aide::axum::routing::get_with(search_entities, |op| {
            op.with(crate::controllers::openapi::extension(
                "x-yorishiro-required-actions",
                serde_json::json!(["search"]),
            ))
        }),
    )
}
