//! Enterprise authentication: tenant-scoped API keys and role checks for enterprise routes.

use crate::YorishiroError;
use crate::controllers::middleware::auth::{self as base_auth, Authenticator};
use crate::db::{AppContextBackend, DbHandle};
use crate::error::ResultExt;
use crate::models::api_keys::{ApiKeyScope, AuthContext, hash_key};
use loco_rs::prelude::async_trait;
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
        app: &loco_rs::app::AppContext,
        presented_key: &str,
        headers: &[(String, String)],
    ) -> Result<AuthContext, YorishiroError> {
        if app.is_sqlite() {
            return base_auth::DefaultAuthenticator
                .authenticate(app, presented_key, headers)
                .await;
        }
        let db = app
            .shared_store
            .get::<DbHandle>()
            .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
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
        use sea_orm::{
            FromQueryResult,
            sea_query::{Alias, Expr, Func, Query},
        };
        #[derive(FromQueryResult)]
        struct Credential {
            id: Uuid,
            workspace_id: Uuid,
            tenant_id: Uuid,
            scope: String,
            user_id: Option<Uuid>,
            audit: bool,
        }
        let query = Query::select()
            .columns(
                [
                    "id",
                    "workspace_id",
                    "tenant_id",
                    "scope",
                    "user_id",
                    "audit",
                ]
                .map(Alias::new),
            )
            .from_function(
                Func::cust(Alias::new("authenticate_api_key"))
                    .args([Expr::val(key_hash), Expr::val(requested)]),
                Alias::new("credential"),
            )
            .to_owned();
        let conn = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
        let row = Credential::find_by_statement(conn.get_database_backend().build(&query))
            .one(&conn)
            .await
            .internal()?;

        let row = row.ok_or(YorishiroError::Unauthenticated)?;
        let Credential {
            id: api_key_id,
            workspace_id,
            tenant_id,
            scope: scope_str,
            user_id,
            audit,
        } = row;

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

        AuthContext {
            api_key_id,
            workspace_id,
            tenant_id,
            scope,
            user_id,
            audit,
        }
        .with_current_membership(&db.identity_connection())
        .await
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

async fn authenticate(
    ctx: &AppContext,
    headers: &HeaderMap,
) -> Result<AuthContext, YorishiroError> {
    let token = bearer_token(headers)?;
    let forwarded = headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), value.to_owned()))
        })
        .collect::<Vec<_>>();
    base_auth::RequestAuth::new(ctx)
        .resolve(token, &forwarded)
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
