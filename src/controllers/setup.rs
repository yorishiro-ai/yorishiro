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
use crate::models::{tenancy, tenant_tenants};

fn max_tenants(ctx: &AppContext) -> Result<Option<i32>, YorishiroError> {
    ctx.shared_store
        .get::<crate::data::tenant_policy::TenantCap>()
        .map(|cap| cap.resolve())
        .ok_or_else(|| YorishiroError::Internal("tenant cap policy missing".into()))
}

#[cfg(feature = "openapi")]
fn status_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/setup/status",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<SetupStatusResponse>()}}})}}),
    )
}
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

#[cfg(feature = "openapi")]
fn setup_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/setup",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<SetupRequest>()}}}, "security": [], "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<SetupResponse>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "409": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
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
    let (created, _, _) =
        tenancy::issue_login_key(&ctx.db, account.user_id, None, Some(account.tenant_id)).await?;

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
    vec![status_doc(), setup_doc()]
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .prefix("setup")
        .add("/", post(setup))
        .add("/status", get(status))
}
