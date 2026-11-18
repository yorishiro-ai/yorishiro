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

#[cfg_attr(feature = "openapi", utoipa::path(
    post,
    path = "/auth/signup",
    request_body = crate::dtos::auth::SignupRequest,
    responses((status = 201, body = crate::dtos::auth::SignupResponse), (status = 422, body = super::openapi::ApiErrorBody)),
    security(()),
    tag = "community"
))]
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

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/auth/root/login", request_body = crate::dtos::auth::RootLoginRequest, responses((status = 200, body = crate::dtos::auth::LoginResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(()), tag = "community"))]
async fn root_login(
    State(ctx): State<AppContext>,
    Json(body): Json<RootLoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let user = user_users::verify_login(&ctx.db, &body.email, &body.password)
        .await?
        .ok_or(YorishiroError::Unauthenticated)?;
    finish_login(&ctx, user, None).await
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/auth/tenant/login", params(("x-tenant-id" = uuid::Uuid, Header)), request_body = crate::dtos::auth::TenantLoginRequest, responses((status = 200, body = crate::dtos::auth::LoginResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(()), tag = "community"))]
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
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_signup),
        super::route_inventory::path_doc(__path_root_login),
        super::route_inventory::path_doc(__path_tenant_login),
    ]
}
