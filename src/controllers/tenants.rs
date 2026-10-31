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

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/tenants/current", responses((status = 200, body = crate::dtos::tenants::TenantRecord), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
async fn detail(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
) -> Result<Json<crate::dtos::tenants::TenantRecord>, ApiError> {
    let tenant = crate::models::tenant_tenants::detail_as(&ctx.db, &actor).await?;
    Ok(Json(crate::dtos::tenants::TenantRecord::render(
        &ctx, tenant,
    )))
}

#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/tenants/current", request_body = crate::dtos::tenants::UpdateTenantRequest, responses((status = 200, body = crate::dtos::tenants::TenantRecord), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
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

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/tenants/current/retirement", responses((status = 200, body = retirement::RetirementInventory), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_owner"]))), tag = "community"))]
async fn inventory(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
) -> Result<Json<retirement::RetirementInventory>, ApiError> {
    Ok(Json(retirement::inventory_as(&ctx.db, &actor).await?))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/tenants/current/disable", request_body = DisableTenantRequest, responses((status = 204, description = "Tenant disabled, credentials revoked and work cancelled; data and identities retained, quota still occupied"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_owner"]))), tag = "community"))]
async fn disable(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<DisableTenantRequest>,
) -> Result<StatusCode, ApiError> {
    retirement::disable_as(&ctx.db, &actor, body.confirmed_tenant_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_detail),
        super::route_inventory::path_doc(__path_update),
        super::route_inventory::path_doc(__path_inventory),
        super::route_inventory::path_doc(__path_disable),
    ]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/tenants/current")
        .add("/", get(detail).put(update))
        .add("/retirement", get(inventory))
        .add("/disable", post(disable))
}
