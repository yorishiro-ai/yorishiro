//! `POST /setup` and `GET /setup/status`: first-run bootstrap, reachable without a bearer token by design, same as `/auth/signup` and `/auth/login`.
//!
//! Creates the deployment's first tenant, workspace and owner account.
//! The resolved edition tenant cap controls whether first-run setup is available.
//! An unlimited cap disables the wizard; the source default is one tenant.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::dtos::setup::{SetupRequest, SetupResponse, SetupStatusResponse};
use crate::error::YorishiroError;
use crate::models::api_keys::IdentityApiKeys;
use crate::models::tenant_memberships::MembershipRole;
use crate::models::{tenancy, tenant_tenants};

fn max_tenants(ctx: &AppContext) -> Result<Option<i32>, YorishiroError> {
    ctx.shared_store
        .get::<crate::data::tenant_policy::TenantCap>()
        .map(|cap| cap.resolve())
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("tenant cap policy missing")))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/setup/status", responses((status = 200, body = crate::dtos::setup::SetupStatusResponse)), security(()), tag = "community"))]
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn status(State(ctx): State<AppContext>) -> Result<Json<SetupStatusResponse>, ApiError> {
    let setup_required = if max_tenants(&ctx)?.is_some() {
        tenant_tenants::count_tenants(&ctx.db).await? == 0
    } else {
        false
    };
    Ok(Json(SetupStatusResponse { setup_required }))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/setup", request_body = crate::dtos::setup::SetupRequest, responses((status = 201, body = crate::dtos::setup::SetupResponse), (status = 404, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(()), tag = "community"))]
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn setup(
    State(ctx): State<AppContext>,
    Json(body): Json<SetupRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let account = tenancy::setup_first_account(
        &ctx.db,
        max_tenants(&ctx)?,
        &body.email,
        &body.password,
        body.display_name.as_deref(),
    )
    .await?;
    let created = IdentityApiKeys::create_api_key(
        &ctx.db,
        account.workspace_id,
        MembershipRole::Owner.max_scope(),
        Some(account.user_id),
        false,
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(SetupResponse {
            user_id: account.user_id,
            email: account.email,
            tenant_id: account.tenant_id,
            workspace_id: account.workspace_id,
            api_key: created.plaintext,
        }),
    ))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_status),
        super::route_inventory::path_doc(__path_setup),
    ]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("setup")
        .add("/", post(setup))
        .add("/status", get(status))
}
