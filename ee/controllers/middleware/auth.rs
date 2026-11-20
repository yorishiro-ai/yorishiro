//! Authentication adapters for enterprise routes.
//!
//! Key resolution itself is edition-neutral and lives on the base `DefaultAuthenticator`, so a tenant-scoped key works identically on community and enterprise routes.

use axum::http::HeaderMap;
use loco_rs::app::AppContext;

use crate::YorishiroError;
use crate::controllers::middleware::auth as base_auth;
use crate::models::api_keys::AuthContext;

/// The header naming which workspace a tenant-scoped API key should act on.
pub use crate::models::api_keys::WORKSPACE_HEADER;

pub(crate) async fn authenticate(
    ctx: &AppContext,
    headers: &HeaderMap,
) -> Result<AuthContext, YorishiroError> {
    let token = base_auth::extract_bearer_key(headers).ok_or(YorishiroError::Unauthenticated)?;
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
/// Use this one whenever the work touches a workspace's own content, since that is what the RLS-scoped connection has to be opened against.
pub(crate) async fn authenticate_workspace(
    ctx: &AppContext,
    headers: &HeaderMap,
) -> Result<AuthContext, YorishiroError> {
    let auth = authenticate(ctx, headers).await?;
    auth.require_workspace()?;
    Ok(auth)
}
