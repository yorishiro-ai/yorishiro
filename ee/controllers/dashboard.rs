//! `GET /api/tenant/overview`: the sole read the admin dashboard's landing page needs, plan, cap, usage counters and the member list, in one round trip.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::openapi::Routes;
use crate::edition::ee::dtos::dashboard::TenantOverview;
use crate::edition::ee::models::tenant_tenants;

async fn tenant_overview(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<TenantOverview>, ApiError> {
    let auth =
        crate::edition::ee::controllers::middleware::auth::authenticate(&ctx, &headers).await?;

    Ok(Json(tenant_tenants::overview_as(&ctx, &auth).await?))
}

pub fn routes() -> Routes {
    Routes::new().tag("enterprise").prefix("api/tenant").add(
        "/overview",
        aide::axum::routing::get_with(tenant_overview, |op| {
            op.with(crate::controllers::openapi::extension(
                "x-yorishiro-required-scopes",
                serde_json::json!(["read"]),
            ))
            .with(crate::controllers::openapi::extension(
                "x-yorishiro-required-actions",
                serde_json::json!(["tenant_get", "tenant_user_list"]),
            ))
            .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
            .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
            .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
        }),
    )
}
