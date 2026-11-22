//! `GET /api/tenant/overview`: the sole read the admin dashboard's landing page needs, plan, cap, usage counters and the member list, in one round trip.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::get;
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::edition::ee::dtos::dashboard::TenantOverview;
use crate::edition::ee::models::tenant_tenants;

#[cfg(feature = "openapi")]
fn tenant_overview_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/tenant/overview",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["tenant_get", "tenant_user_list"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<TenantOverview>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn tenant_overview(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<TenantOverview>, ApiError> {
    let auth =
        crate::edition::ee::controllers::middleware::auth::authenticate(&ctx, &headers).await?;

    Ok(Json(tenant_tenants::overview_as(&ctx, &auth).await?))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/tenant")
        .add("/overview", get(tenant_overview))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::openapi::route_inventory::RouteDoc> {
    vec![tenant_overview_doc()]
}
