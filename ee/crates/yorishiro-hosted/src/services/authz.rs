//! Role-based authorization for this crate's own routes, orthogonal to `ApiKeyScope`.

use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use loco_rs::app::AppContext;
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::db::DbHandle;
use yorishiro_core::models::tenancy::{self, MembershipRole};
use yorishiro_core::services::auth;
use yorishiro_core::services::auth::Authenticator;

/// The prefix-stripping and the empty-credential check both live in [`auth::bearer_credential`], so this path and the ones upstream cannot disagree about what `Authorization: Bearer ` means.
fn bearer_token(headers: &HeaderMap) -> Result<&str, YorishiroError> {
    auth::bearer_credential(headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()))
        .ok_or(YorishiroError::Unauthenticated)
}

fn db_handle(ctx: &AppContext) -> Result<DbHandle, YorishiroError> {
    ctx.shared_store
        .get::<DbHandle>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))
}

/// Authenticates the bearer API key and returns the full context, **workspace included**.
///
/// Goes through [`crate::services::tenant_auth::TenantScopedAuthenticator`], the same seam every authenticated path in this process resolves through, so both key kinds work on these routes: a workspace-scoped key names its own workspace, and a tenant-scoped one names it per request with `X-Workspace-Id`.
/// Resolving it any other way here would make a REST route and an MCP tool disagree about who the caller is.
///
/// [`authenticate_tenant`] is the weaker form for routes that need only the tenant.
/// Use this one whenever the work touches a workspace's own content, since that is what the RLS-scoped connection has to be opened against.
pub(crate) async fn authenticate_workspace(
    ctx: &AppContext,
    headers: &HeaderMap,
) -> Result<auth::AuthContext, YorishiroError> {
    let token = bearer_token(headers)?;
    let db = db_handle(ctx)?;
    let forwarded: Vec<(String, String)> = headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), value.to_owned()))
        })
        .collect();
    crate::services::tenant_auth::TenantScopedAuthenticator
        .authenticate(&db, token, &forwarded)
        .await
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
    let token = bearer_token(headers)?;
    let db = db_handle(ctx)?;
    let auth_ctx = auth::authenticate(&db, token).await?;
    Ok((auth_ctx.tenant_id, auth_ctx.user_id))
}

/// Authenticates the bearer API key and requires the attributed user to hold an Owner/Admin membership in the key's tenant.
///
/// This is a **role-based** check (orthogonal to `ApiKeyScope`): a Member-role key can hold `write` scope for content operations while still having no business reading billing data.
///
/// Service-only API keys (no `user_id`) are rejected because admin status can only be determined from a user's tenant membership.
pub(crate) async fn authenticate_tenant_admin(
    ctx: &AppContext,
    headers: &HeaderMap,
) -> Result<Uuid, YorishiroError> {
    let token = bearer_token(headers)?;
    let db = db_handle(ctx)?;
    let auth_ctx = auth::authenticate(&db, token).await?;
    let user_id = auth_ctx.user_id.ok_or(YorishiroError::Unauthenticated)?;
    tenancy::get_membership_role(&ctx.db, auth_ctx.tenant_id, user_id)
        .await?
        .filter(|role| matches!(role, MembershipRole::Owner | MembershipRole::Admin))
        .ok_or_else(|| YorishiroError::ScopeInsufficient {
            message: "the hosted dashboard is restricted to tenant owners/admins".into(),
            hint: "ask a tenant owner to grant you the admin role".into(),
        })?;
    Ok(auth_ctx.tenant_id)
}
