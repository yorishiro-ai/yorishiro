use axum::Json;
use axum::extract::State;
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
use crate::dtos::tenants::DisableTenantRequest;
use crate::models::tenant_tenants::retirement;

async fn detail(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
) -> Result<Json<crate::dtos::tenants::TenantRecord>, ApiError> {
    let tenant = crate::models::tenant_tenants::detail_as(&ctx.db, &actor).await?;
    Ok(Json(crate::dtos::tenants::TenantRecord::render(
        &ctx, tenant,
    )))
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

async fn inventory(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
) -> Result<Json<retirement::RetirementInventory>, ApiError> {
    Ok(Json(retirement::inventory_as(&ctx.db, &actor).await?))
}

async fn disable(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<DisableTenantRequest>,
) -> Result<axum::response::NoContent, ApiError> {
    retirement::disable_as(&ctx.db, &actor, body.confirmed_tenant_id).await?;
    Ok(axum::response::NoContent)
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/tenants/current")
        .add(
            "/",
            aide::axum::routing::get_with(detail, |op| {
                op.id("GetTenant").security_requirement("bearer_auth")
            }),
        )
        .add(
            "/",
            aide::axum::routing::put_with(update, |op| {
                op.id("UpdateTenant").security_requirement("bearer_auth")
            }),
        )
        .add(
            "/retirement",
            aide::axum::routing::get_with(inventory, |op| {
                op.id("GetTenantRetirementInventory")
                    .security_requirement("bearer_auth")
            }),
        )
        .add(
            "/disable",
            aide::axum::routing::post_with(disable, |op| {
                op.id("DisableTenant").security_requirement("bearer_auth")
            }),
        )
}
