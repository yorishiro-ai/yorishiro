//! Enterprise authentication: tenant-scoped API keys and role checks for enterprise routes.

use crate::YorishiroError;
use crate::controllers::middleware::auth::{self as base_auth, Authenticator};
use crate::db::DbHandle;
use crate::error::ResultExt;
use crate::models::api_keys::{ApiKeyScope, AuthContext, hash_key};
use async_trait::async_trait;
use uuid::Uuid;

/// The header naming which workspace a tenant-scoped API key should act on.
pub const WORKSPACE_HEADER: &str = "x-workspace-id";

pub(crate) struct TenantScopedAuthenticator;

/// The outcome of reading the workspace header.
enum RequestedWorkspace {
    Absent,
    Present(Uuid),
    /// Present but not a UUID.
    /// Distinct from `Absent` on purpose: treating an unparseable value as "not sent" would send a request meant for one workspace to whichever one the key happens to carry.
    Malformed,
}

fn requested_workspace(headers: &[(String, String)]) -> RequestedWorkspace {
    match headers
        .iter()
        .find(|(name, _)| name == WORKSPACE_HEADER)
        .map(|(_, value)| value.trim())
    {
        None => RequestedWorkspace::Absent,
        Some(value) => match Uuid::parse_str(value) {
            Ok(id) => RequestedWorkspace::Present(id),
            Err(_) => RequestedWorkspace::Malformed,
        },
    }
}

#[async_trait]
impl Authenticator for TenantScopedAuthenticator {
    async fn authenticate(
        &self,
        db: &DbHandle,
        presented_key: &str,
        headers: &[(String, String)],
    ) -> Result<AuthContext, YorishiroError> {
        let pool = db.tenant.pool();
        let requested = match requested_workspace(headers) {
            RequestedWorkspace::Absent => None,
            RequestedWorkspace::Present(id) => Some(id),
            RequestedWorkspace::Malformed => {
                return Err(YorishiroError::ValidationFailed {
                    message: format!("{WORKSPACE_HEADER} is not a valid UUID"),
                    details: Vec::new(),
                    hint: "send the workspace's UUID, or omit the header to use a \
                           workspace-scoped key"
                        .into(),
                });
            }
        };

        let key_hash = hash_key(presented_key);

        // The two-argument overload of the `authenticate_api_key` SECURITY DEFINER function the schema migration creates.
        // `p_requested_workspace` is only consulted for a key with no workspace of its own, and resolves only when the named workspace belongs to that key's tenant: the tenant isolation boundary for these keys.
        let row: Option<(Uuid, Uuid, Uuid, String, Option<Uuid>, bool)> = sqlx::query_as(
            "SELECT id, workspace_id, tenant_id, scope, user_id, audit \
             FROM authenticate_api_key($1, $2)",
        )
        .bind(key_hash)
        .bind(requested)
        .fetch_optional(pool)
        .await
        .internal()?;

        let (api_key_id, workspace_id, tenant_id, scope_str, user_id, audit) =
            row.ok_or(YorishiroError::Unauthenticated)?;

        // A workspace-scoped key ignores the header, so a client that sends one naming a different workspace is asking for something it will not get.
        // Rejecting says so; proceeding would act on the key's own workspace instead: a write landing where the client never named, answered with a 2xx.
        if let Some(requested) = requested
            && requested != workspace_id
        {
            return Err(YorishiroError::ValidationFailed {
                message: format!("{WORKSPACE_HEADER} names a workspace this key cannot act on"),
                details: Vec::new(),
                hint: "this key is scoped to a single workspace; omit the header, or use a \
                       tenant-scoped key to choose a workspace per request"
                    .into(),
            });
        }

        let scope = ApiKeyScope::from_db_str(&scope_str).ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!(
                "unknown api key scope in database: {scope_str}"
            ))
        })?;

        Ok(AuthContext {
            api_key_id,
            workspace_id,
            tenant_id,
            scope,
            user_id,
            audit,
        })
    }
}

use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use loco_rs::app::AppContext;

/// The prefix-stripping and the empty-credential check both live in [`base_auth::bearer_credential`], so this path and the ones upstream cannot disagree about what `Authorization: Bearer ` means.
fn bearer_token(headers: &HeaderMap) -> Result<&str, YorishiroError> {
    base_auth::bearer_credential(headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()))
        .ok_or(YorishiroError::Unauthenticated)
}

fn db_handle(ctx: &AppContext) -> Result<DbHandle, YorishiroError> {
    ctx.shared_store
        .get::<DbHandle>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))
}

async fn authenticate(
    ctx: &AppContext,
    headers: &HeaderMap,
) -> Result<AuthContext, YorishiroError> {
    let token = bearer_token(headers)?;
    if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return crate::models::api_keys::Entity::authenticate_sqlite(&ctx.db, token).await;
    }
    let db = db_handle(ctx)?;
    let forwarded = headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), value.to_owned()))
        })
        .collect::<Vec<_>>();
    crate::controllers::extractors::authenticator(ctx)
        .map_err(|error| error.0)?
        .authenticate(&db, token, &forwarded)
        .await
}

/// Authenticates the bearer API key and returns the full context, **workspace included**.
///
/// Goes through [`crate::edition::ee::controllers::middleware::auth::TenantScopedAuthenticator`], the same seam every authenticated path in this process resolves through, so both key kinds work on these routes: a workspace-scoped key names its own workspace, and a tenant-scoped one names it per request with `X-Workspace-Id`.
/// Resolving it any other way here would make a REST route and an MCP tool disagree about who the caller is.
///
/// [`authenticate_tenant`] is the weaker form for routes that need only the tenant.
/// Use this one whenever the work touches a workspace's own content, since that is what the RLS-scoped connection has to be opened against.
pub(crate) async fn authenticate_workspace(
    ctx: &AppContext,
    headers: &HeaderMap,
) -> Result<AuthContext, YorishiroError> {
    authenticate(ctx, headers).await
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

/// Authenticates the bearer API key and requires the attributed user to hold an Owner/Admin membership in the key's tenant.
///
/// This is a **role-based** check (orthogonal to `ApiKeyScope`): a Member-role key can hold `write` scope for content operations while still having no business reading billing data.
///
/// Service-only API keys (no `user_id`) are rejected because admin status can only be determined from a user's tenant membership.
pub(crate) async fn authenticate_tenant_admin(
    ctx: &AppContext,
    headers: &HeaderMap,
) -> Result<Uuid, YorishiroError> {
    let auth_ctx = authenticate(ctx, headers).await?;
    let user_id = auth_ctx.user_id.ok_or(YorishiroError::Unauthenticated)?;
    crate::models::tenant_memberships::get_membership_role(&ctx.db, auth_ctx.tenant_id, user_id)
        .await?
        .filter(|role| role.administers_tenant())
        .ok_or_else(|| YorishiroError::ScopeInsufficient {
            message: "the hosted dashboard is restricted to tenant owners/admins".into(),
            hint: "ask a tenant owner to grant you the admin role".into(),
        })?;
    Ok(auth_ctx.tenant_id)
}
