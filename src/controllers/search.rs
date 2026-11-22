use axum::Json;
use axum::extract::{Query, State};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::{ReadScope, Verified, search_token_limiter};
use crate::dtos::search::SearchEntitiesParams;
use crate::models::search::{self, SearchHit};

#[cfg(feature = "openapi")]
fn search_entities_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/search",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["search"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<SearchHit>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "503": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "query_text", "in": "query", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}, {"name": "entity_type", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}, {"name": "filter", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}, {"name": "limit", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i64>()}]}),
    )
}
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
    vec![search_entities_doc()]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/search")
        .add("/", axum::routing::get(search_entities))
}
