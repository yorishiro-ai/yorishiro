use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::db::DbHandle;
use yorishiro_core::models::tenancy::{self, MembershipRole};
use yorishiro_core::services::auth::{self, Authenticator};

use crate::state::HostedState;

/// The prefix-stripping and the empty-credential check both live in [`auth::bearer_credential`], so this path and the ones upstream cannot disagree about what `Authorization: Bearer ` means.
/// This function stays because the upstream one returns an `Option`: the mapping to `Unauthenticated` is this crate's business, not core's.
fn bearer_token(headers: &HeaderMap) -> Result<&str, YorishiroError> {
    auth::bearer_credential(headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()))
        .ok_or(YorishiroError::Unauthenticated)
}

/// `ee/` is Postgres-only (an LLM-calling, billing-integrated deployment has no single-tenant Sqlite story), so this always builds the Postgres arm.
/// [`auth::authenticate`] takes the whole [`DbHandle`] because it also has to run before a workspace is known on engines that need one; this crate just always hands it the one engine it runs on.
fn db_handle(state: &HostedState) -> DbHandle {
    DbHandle::Postgres {
        tenant: state.tenant_db.clone(),
        identity: state.identity_pool.clone(),
    }
}

/// Authenticates the bearer API key and returns the full context, **workspace included**.
///
/// Goes through [`TenantScopedAuthenticator`], the same seam every authenticated path in this process resolves through, so both key kinds work on these routes: a workspace-scoped key names its own workspace, and a tenant-scoped one names it per request with `X-Workspace-Id`.
/// Resolving it any other way here would make a REST route and an MCP tool disagree about who the caller is.
///
/// [`authenticate_tenant`] is the weaker form for routes that need only the tenant.
/// Use this one whenever the work touches a workspace's own content, since that is what the RLS-scoped connection from `state.tenant_db` has to be opened against.
pub(crate) async fn authenticate_workspace(
    state: &HostedState,
    headers: &HeaderMap,
) -> Result<auth::AuthContext, YorishiroError> {
    let token = bearer_token(headers)?;
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
        .authenticate(&db_handle(state), token, &forwarded)
        .await
}

/// Authenticates the bearer API key and returns the tenant it belongs to, with **no role requirement**.
///
/// The marketplace is the caller: publishing a version, reviewing and forking are all per-tenant acts that any valid key for that tenant may perform.
/// Requiring Owner/Admin here would silently narrow the feature rather than relocate it.
///
/// Ownership is still enforced downstream: the service scopes every write by this `tenant_id`, and acting on another tenant's template answers `404` rather than `403`.
///
/// Returns the attributed `user_id` alongside it, which is `None` for a service-only key.
/// Publishing a version, reviewing and forking all record an author, and a key with no user behind it records none.
pub(crate) async fn authenticate_tenant(
    state: &HostedState,
    headers: &HeaderMap,
) -> Result<(Uuid, Option<Uuid>), YorishiroError> {
    let token = bearer_token(headers)?;
    let ctx = auth::authenticate(&db_handle(state), token).await?;
    Ok((ctx.tenant_id, ctx.user_id))
}

/// Authenticates the bearer API key and requires the attributed user to hold an Owner/Admin membership in the key's tenant.
/// This is a **role-based** check (orthogonal to `ApiKeyScope`): a Member-role key can hold `write` scope for content operations while still having no business reading billing data.
///
/// Service-only API keys (no `user_id`) are rejected because admin status can only be determined from a user's tenant membership.
pub(crate) async fn authenticate_tenant_admin(
    state: &HostedState,
    headers: &HeaderMap,
) -> Result<Uuid, YorishiroError> {
    let token = bearer_token(headers)?;
    let ctx = auth::authenticate(&db_handle(state), token).await?;
    let user_id = ctx.user_id.ok_or(YorishiroError::Unauthenticated)?;
    tenancy::get_membership_role(&state.identity_pool, ctx.tenant_id, user_id)
        .await?
        .filter(|role| matches!(role, MembershipRole::Owner | MembershipRole::Admin))
        .ok_or_else(|| YorishiroError::ScopeInsufficient {
            message: "the hosted dashboard is restricted to tenant owners/admins".into(),
            hint: "ask a tenant owner to grant you the admin role".into(),
        })?;
    Ok(ctx.tenant_id)
}

#[cfg(test)]
#[path = "../../tests/services/authz.rs"]
mod tests;
