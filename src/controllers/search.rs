use axum::Json;
use axum::extract::{Query, State};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::{ReadScope, Verified, search_token_limiter};
use crate::dtos::search::SearchEntitiesParams;
use crate::models::search::{self, SearchHit};

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/search", params(("query_text" = String, Query, description = "Text to embed and search for"), ("entity_type" = Option<String>, Query), ("filter" = Option<String>, Query, description = "JSON-encoded containment filter"), ("limit" = Option<i64>, Query)), responses((status = 200, body = [SearchHit]), (status = 401, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody), (status = 503, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["search"]))), tag = "community"))]
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

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![super::openapi::route_inventory::path_doc(
        __path_search_entities,
    )]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/search")
        .add("/", axum::routing::get(search_entities))
}
