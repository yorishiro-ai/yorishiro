//! Enterprise authentication: role checks for enterprise routes.
//!
//! Key resolution itself is edition-neutral and lives on the base `DefaultAuthenticator`, so a tenant-scoped key works identically on community and enterprise routes.

use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::YorishiroError;
use crate::controllers::middleware::auth as base_auth;
use crate::models::api_keys::AuthContext;

/// The header naming which workspace a tenant-scoped API key should act on.
pub use crate::models::api_keys::WORKSPACE_HEADER;

/// The prefix-stripping and the empty-credential check both live in [`base_auth::bearer_credential`], so this path and the ones upstream cannot disagree about what `Authorization: Bearer ` means.
fn bearer_token(headers: &HeaderMap) -> Result<&str, YorishiroError> {
    base_auth::bearer_credential(headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()))
        .ok_or(YorishiroError::Unauthenticated)
}

pub(crate) async fn authenticate(
    ctx: &AppContext,
    headers: &HeaderMap,
) -> Result<AuthContext, YorishiroError> {
    let token = bearer_token(headers)?;
    let forwarded = base_auth::header_pairs(headers)?;
    base_auth::RequestAuth::new(ctx)
        .resolve(token, &forwarded)
        .await
}

/// Authenticates the bearer API key and returns the full context, **workspace included**.
///
/// Goes through the installed authenticator, the same seam every authenticated path in this process resolves through, so both key kinds work on these routes: a workspace-scoped key names its own workspace, and a tenant-scoped one names it per request with `X-Workspace-Id`.
/// Resolving it any other way here would make a REST route and an MCP tool disagree about who the caller is.
///
/// [`authenticate_tenant`] is the weaker form for routes that need only the tenant.
/// Use this one whenever the work touches a workspace's own content, since that is what the RLS-scoped connection has to be opened against.
pub(crate) async fn authenticate_workspace(
    ctx: &AppContext,
    headers: &HeaderMap,
) -> Result<AuthContext, YorishiroError> {
    let auth = authenticate(ctx, headers).await?;
    auth.require_workspace()?;
    Ok(auth)
}

/// Authenticates the bearer API key and returns the tenant it belongs to, with **no role requirement**.
///
/// The marketplace is the caller: publishing a version, reviewing and forking are all per-tenant acts that any valid key for that tenant may perform.
///
/// Ownership is still enforced downstream: the service scopes every write by this `tenant_id`, and acting on another tenant's template answers `404` rather than `403`.
///
/// Returns the attributed `user_id` alongside it, which is `None` for a service-only key.
pub(crate) async fn authenticate_tenant(
    ctx: &AppContext,
    headers: &HeaderMap,
) -> Result<(Uuid, Option<Uuid>), YorishiroError> {
    let auth_ctx = authenticate(ctx, headers).await?;
    Ok((auth_ctx.tenant_id, auth_ctx.user_id))
}
