//! `GET /api/tenant/overview`: the sole read the admin dashboard's landing page needs, plan, cap, usage counters and the member list, in one round trip.

use crate::controllers::extractors::AuthContext;
use axum::Json;
use axum::extract::State;
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::openapi::Routes;
use crate::edition::ee::dtos::dashboard::TenantOverview;
use crate::edition::ee::models::tenant_tenants;

async fn tenant_overview(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
) -> Result<Json<TenantOverview>, ApiError> {
    Ok(Json(tenant_tenants::overview_as(&ctx, &auth).await?))
}

pub fn routes() -> Routes {
    Routes::new().tag("enterprise").prefix("api/tenant").add(
        "/overview",
        aide::axum::routing::get_with(tenant_overview, |op| {
            op.with(crate::controllers::openapi::extension(
                "x-yorishiro-required-actions",
                serde_json::json!(["tenant_get", "tenant_user_list"]),
            ))
        }),
    )
}
