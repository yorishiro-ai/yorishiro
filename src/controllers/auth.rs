//! Public account registration and credential-based API key issuance.
//!
//! Both run on `ctx.db` (Loco's own connection, migration role, no RLS scope), same as the admin CLI tasks: no tenant/workspace context exists yet for RLS to scope by.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
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

/// # Panics
/// Panics if the static tenant header metadata is invalid.
pub fn routes() -> Routes {
    Routes::new()
        .prefix("auth")
        .add(
            "/signup",
            aide::axum::routing::post_with(signup, |op| {
                op.tag("community")
                    .response::<201, Json<SignupResponse>>()
                    .response::<422, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/root/login",
            aide::axum::routing::post_with(root_login, |op| {
                op.tag("community")
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<403, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/tenant/login",
            aide::axum::routing::post_with(tenant_login, |op| {
                op.tag("community")
                    .with(|mut op| {
                        op.inner_mut().parameters.push(aide::openapi::ReferenceOr::Item(
                            serde_json::from_value(serde_json::json!({"in": "header", "name": "x-tenant-id", "required": true, "schema": {"type": "string", "format": "uuid"}})).expect("tenant login header"),
                        ));
                        op
                    })
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<403, Json<super::openapi::ApiErrorBody>>()
            }),
        )
}
