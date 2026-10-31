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

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/tenant/overview", responses((status = 200, body = crate::edition::ee::controllers::openapi::TenantOverview), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "enterprise"))]
async fn tenant_overview(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<TenantOverview>, ApiError> {
    let auth =
        crate::edition::ee::controllers::middleware::auth::authenticate_workspace(&ctx, &headers)
            .await?;

    Ok(Json(tenant_tenants::overview_as(&ctx, &auth).await?))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/tenant")
        .add("/overview", get(tenant_overview))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::route_inventory::RouteDoc> {
    vec![crate::controllers::route_inventory::path_doc(
        __path_tenant_overview,
    )]
}
