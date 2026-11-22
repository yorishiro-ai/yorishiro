//! Public account registration and credential-based API key issuance.
//!
//! Both run on `ctx.db` (Loco's own connection, migration role, no RLS scope), same as the admin CLI tasks: no tenant/workspace context exists yet for RLS to scope by.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::dtos::auth::{
    LoginResponse, RootLoginRequest, SignupRequest, SignupResponse, TenantLoginRequest,
};
use crate::error::YorishiroError;
use crate::models::{tenancy, user_users};

#[cfg(feature = "openapi")]
fn signup_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/auth/signup",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<SignupRequest>()}}}, "security": [], "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<SignupResponse>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
/// Creates a brand-new tenant whose root is the caller; there is no workspace yet.
///
/// The new tenant can be managed using a tenant-scoped login key.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn signup(
    State(ctx): State<AppContext>,
    Json(body): Json<SignupRequest>,
) -> Result<(StatusCode, Json<SignupResponse>), ApiError> {
    let max_tenants = ctx
        .shared_store
        .get::<crate::data::tenant_policy::TenantCap>()
        .ok_or_else(|| YorishiroError::Internal("tenant cap policy missing".into()))?
        .resolve();
    let (user, tenant_id) = tenancy::signup_account(
        &ctx.db,
        max_tenants,
        &body.email,
        &body.password,
        body.display_name.as_deref(),
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(SignupResponse {
            user_id: user.id,
            email: user.root_email(),
            tenant_id,
            workspaces: vec![],
        }),
    ))
}

#[cfg(feature = "openapi")]
fn root_login_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/auth/root/login",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<RootLoginRequest>()}}}, "security": [], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<LoginResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn root_login(
    State(ctx): State<AppContext>,
    Json(body): Json<RootLoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let user = user_users::verify_login(&ctx.db, &body.email, &body.password)
        .await?
        .ok_or(YorishiroError::Unauthenticated)?;
    finish_login(&ctx, user, None).await
}

#[cfg(feature = "openapi")]
fn tenant_login_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/auth/tenant/login",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<TenantLoginRequest>()}}}, "security": [], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<LoginResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "x-tenant-id", "in": "header", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
async fn tenant_login(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(body): Json<TenantLoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let mut tenant_headers = headers.get_all("x-tenant-id").iter();
    let tenant_id = tenant_headers
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .ok_or(YorishiroError::Unauthenticated)?;
    if tenant_headers.next().is_some() {
        return Err(YorishiroError::Unauthenticated.into());
    }
    let user = user_users::verify_tenant_login(&ctx.db, tenant_id, &body.user_name, &body.password)
        .await?
        .ok_or(YorishiroError::Unauthenticated)?;
    finish_login(&ctx, user, Some(tenant_id)).await
}

async fn finish_login(
    ctx: &AppContext,
    user: user_users::Model,
    expected_tenant: Option<uuid::Uuid>,
) -> Result<Json<LoginResponse>, ApiError> {
    let hash = user
        .password_hash
        .as_deref()
        .ok_or(YorishiroError::Unauthenticated)?;
    let (created, tenant_id, scope) =
        tenancy::issue_login_key(&ctx.db, user.id, Some(hash), expected_tenant).await?;
    Ok(Json(LoginResponse {
        api_key: created.plaintext,
        api_key_id: created.id,
        tenant_id,
        scope,
        user_id: user.id,
    }))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("auth")
        .add("/signup", post(signup))
        .add("/root/login", post(root_login))
        .add("/tenant/login", post(tenant_login))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![signup_doc(), root_login_doc(), tenant_login_doc()]
}
