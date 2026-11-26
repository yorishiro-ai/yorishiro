//! Public account registration and credential-based access key issuance.
//!
//! Both run on `ctx.db` (Loco's own connection, migration role, no RLS scope), same as the admin CLI tasks: no tenant/workspace context exists yet for RLS to scope by.

use crate::controllers::Created;
use axum::Json;
use axum::extract::State;
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::openapi::Routes;
use crate::dtos::auth::{
    LoginResponse, RootLoginRequest, SignupRequest, SignupResponse, TenantLoginRequest,
};
use crate::error::YorishiroError;
use crate::models::{tenancy, user_users};

/// Creates a brand-new tenant whose root is the caller; there is no workspace yet.
///
/// The new tenant can be managed using a tenant-scoped login key.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn signup(
    State(ctx): State<AppContext>,
    Json(body): Json<SignupRequest>,
) -> Result<Created<SignupResponse>, ApiError> {
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

    Ok(Created(SignupResponse {
        user_id: user.id,
        email: user.root_email(),
        tenant_id,
        workspaces: vec![],
    }))
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

async fn tenant_login(
    State(ctx): State<AppContext>,
    Json(body): Json<TenantLoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let tenant_id = body.tenant_id;
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
    let (created, tenant_id) =
        tenancy::issue_login_key(&ctx.db, user.id, Some(hash), expected_tenant).await?;
    Ok(Json(LoginResponse {
        access_key: created.plaintext,
        access_key_id: created.id,
        tenant_id,
        user_id: user.id,
    }))
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("auth")
        .add("/signup", aide::axum::routing::post(signup))
        .add("/root/login", aide::axum::routing::post(root_login))
        .add("/tenant/login", aide::axum::routing::post(tenant_login))
}
