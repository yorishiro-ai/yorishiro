use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
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
) -> Result<StatusCode, ApiError> {
    retirement::disable_as(&ctx.db, &actor, body.confirmed_tenant_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn routes() -> Routes {
    Routes::new().tag("community")
        .prefix("api/tenants/current")
        .add("/", aide::axum::routing::get_with(detail, |op| op.security_requirement("bearer_auth").with(crate::controllers::openapi::extension("x-yorishiro-required-iam-action", serde_json::json!("tenant_get"))).response::<401, Json<super::openapi::ApiErrorBody>>().response::<403, Json<super::openapi::ApiErrorBody>>())).add("/", aide::axum::routing::put_with(update, |op| op.security_requirement("bearer_auth").with(crate::controllers::openapi::extension("x-yorishiro-required-iam-action", serde_json::json!("tenant_update"))).response::<401, Json<super::openapi::ApiErrorBody>>().response::<403, Json<super::openapi::ApiErrorBody>>().response::<409, Json<super::openapi::ApiErrorBody>>()))
        .add("/retirement", aide::axum::routing::get_with(inventory, |op| op.security_requirement("bearer_auth").with(crate::controllers::openapi::extension("x-yorishiro-required-scopes", serde_json::json!(["read"]))).with(crate::controllers::openapi::extension("x-yorishiro-required-actions", serde_json::json!(["tenant_get"]))).response::<401, Json<super::openapi::ApiErrorBody>>().response::<403, Json<super::openapi::ApiErrorBody>>()))
        .add("/disable", aide::axum::routing::post_with(disable, |op| op.security_requirement("bearer_auth").with(crate::controllers::openapi::extension("x-yorishiro-required-scopes", serde_json::json!(["read"]))).with(crate::controllers::openapi::extension("x-yorishiro-required-actions", serde_json::json!(["tenant_update"]))).response_with::<204, (), _>(|op| op.description("Tenant disabled, credentials revoked and work cancelled; data and identities retained, quota still occupied")).response::<401, Json<super::openapi::ApiErrorBody>>().response::<403, Json<super::openapi::ApiErrorBody>>().response::<422, Json<super::openapi::ApiErrorBody>>()))
}
