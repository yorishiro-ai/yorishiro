use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::dtos::tenants::DisableTenantRequest;
use crate::models::tenant_tenants::retirement;

#[cfg(feature = "openapi")]
fn detail_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/tenants/current",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-iam-action" :  serde_json::json!("tenant_get"), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::dtos::tenants::TenantRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn detail(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
) -> Result<Json<crate::dtos::tenants::TenantRecord>, ApiError> {
    let tenant = crate::models::tenant_tenants::detail_as(&ctx.db, &actor).await?;
    Ok(Json(crate::dtos::tenants::TenantRecord::render(
        &ctx, tenant,
    )))
}

#[cfg(feature = "openapi")]
fn update_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/tenants/current",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::dtos::tenants::UpdateTenantRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-iam-action" :  serde_json::json!("tenant_update"), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::dtos::tenants::TenantRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "409": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn update(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(input): Json<crate::dtos::tenants::UpdateTenantRequest>,
) -> Result<Json<crate::dtos::tenants::TenantRecord>, ApiError> {
    let tenant = crate::models::tenant_tenants::update_as(&ctx.db, &actor, input).await?;
    Ok(Json(crate::dtos::tenants::TenantRecord::render(
        &ctx, tenant,
    )))
}

#[cfg(feature = "openapi")]
fn inventory_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/tenants/current/retirement",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["tenant_get"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<retirement::RetirementInventory>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn inventory(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
) -> Result<Json<retirement::RetirementInventory>, ApiError> {
    Ok(Json(retirement::inventory_as(&ctx.db, &actor).await?))
}

#[cfg(feature = "openapi")]
fn disable_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/tenants/current/disable",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<DisableTenantRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["tenant_update"]), "responses": {"204": serde_json::json!({"description": "Tenant disabled, credentials revoked and work cancelled; data and identities retained, quota still occupied"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn disable(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<DisableTenantRequest>,
) -> Result<StatusCode, ApiError> {
    retirement::disable_as(&ctx.db, &actor, body.confirmed_tenant_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![detail_doc(), update_doc(), inventory_doc(), disable_doc()]
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .prefix("api/tenants/current")
        .add("/", get(detail).put(update))
        .add("/retirement", get(inventory))
        .add("/disable", post(disable))
}
