//! `POST /setup` and `GET /setup/status`: first-run bootstrap, reachable without a bearer token by design, same as `/auth/signup` and `/auth/root/login`.
//!
//! Creates the deployment's first tenant and admin account, then issues a tenant-scoped admin key.
//! The admin creates and selects workspaces explicitly.
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
use crate::models::api_keys::{ApiKeyScope, IdentityApiKeys};
use crate::models::{tenancy, tenant_tenants};

fn max_tenants(ctx: &AppContext) -> Result<Option<i32>, YorishiroError> {
    ctx.shared_store
        .get::<crate::data::tenant_policy::TenantCap>()
        .map(|cap| cap.resolve())
        .ok_or_else(|| YorishiroError::Internal("tenant cap policy missing".into()))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/setup/status", responses((status = 200, body = SetupStatusResponse)), security(()), tag = "community"))]
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn status(State(ctx): State<AppContext>) -> Result<Json<SetupStatusResponse>, ApiError> {
    let setup_required = if max_tenants(&ctx)?.is_some() {
        tenant_tenants::count_tenants(&ctx.db).await? == 0
    } else {
        false
    };
    Ok(Json(SetupStatusResponse { setup_required }))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/setup", request_body = SetupRequest, responses((status = 201, body = SetupResponse), (status = 404, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(()), tag = "community"))]
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn setup(
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
    // Operator bootstrap issues the first audit-reading key; later grants require an authorized issuer.
    // The scope is the top of the credential ladder, independent of IAM permissions.
    let created = IdentityApiKeys::create_tenant_api_key(
        &ctx.db,
        account.tenant_id,
        ApiKeyScope::Migration,
        Some(account.user_id),
        true,
        "initial-admin",
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(SetupResponse {
            user_id: account.user_id,
            email: account.email,
            tenant_id: account.tenant_id,
            api_key: created.plaintext,
        }),
    ))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![
        super::openapi::route_inventory::path_doc(__path_status),
        super::openapi::route_inventory::path_doc(__path_setup),
    ]
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .prefix("setup")
        .add("/", post(setup))
        .add("/status", get(status))
}
