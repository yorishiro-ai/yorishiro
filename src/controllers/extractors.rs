use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRef, FromRequestParts};
use axum::http::header;
use axum::http::request::Parts;
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::db::DbHandle;
use crate::error::YorishiroError;
use crate::services::auth;
use crate::services::auth::{ApiKeyScope, Authenticator};

use super::ApiError;

/// Emits a `warn` for a request rejected before it reaches a handler (bad/missing key, or insufficient scope).
/// The presented key is never logged: only the caller IP (when `ConnectInfo` is present), the path, and the reason.
fn log_auth_rejection(parts: &Parts, err: &YorishiroError) {
    let client = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    tracing::warn!(client = %client, path = %parts.uri.path(), error = %err, "request rejected during authentication");
}

/// Copies the request's headers into the shape [`auth::Authenticator`] takes.
/// Headers whose value cannot be read as UTF-8 are dropped rather than failing the request.
fn header_pairs(parts: &Parts) -> Vec<(String, String)> {
    parts
        .headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), value.to_owned()))
        })
        .collect()
}

/// Shared by every extractor in this file.
fn extract_bearer_key(parts: &Parts) -> Result<&str, ApiError> {
    auth::bearer_credential(
        parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
    )
    .ok_or_else(|| {
        let err = YorishiroError::Unauthenticated;
        log_auth_rejection(parts, &err);
        ApiError(err)
    })
}

/// Also used by the MCP adapter (`services::mcp`), which authorizes per-tool rather than through this file's `FromRequestParts` impls, but still needs the same `DbHandle` out of `shared_store`.
pub(crate) fn db_handle(ctx: &AppContext) -> Result<DbHandle, ApiError> {
    ctx.shared_store.get::<DbHandle>().ok_or_else(|| {
        ApiError(YorishiroError::Internal(anyhow::anyhow!(
            "DbHandle missing"
        )))
    })
}

/// See `db_handle`'s doc comment: also used by `services::mcp`.
pub(crate) fn authenticator(ctx: &AppContext) -> Result<Arc<dyn Authenticator>, ApiError> {
    ctx.shared_store
        .get::<Arc<dyn Authenticator>>()
        .ok_or_else(|| {
            ApiError(YorishiroError::Internal(anyhow::anyhow!(
                "Authenticator missing"
            )))
        })
}

/// See `db_handle`'s doc comment: also used by `services::mcp`.
/// Returns the deployment-wide provider, ignoring any workspace-level assignment: the caller has no `workspace_id` yet (setup, a fresh workspace's dimension stamp) or explicitly wants the deployment default regardless of what a workspace is assigned.
/// A caller resolving a provider *for* a workspace's own work (search, embedding sync) wants `resolve_embedding_provider` instead.
pub(crate) fn embedding_provider(
    ctx: &AppContext,
) -> Result<Arc<dyn crate::services::embedding::EmbeddingProvider>, ApiError> {
    ctx.shared_store
        .get::<Arc<dyn crate::services::embedding::EmbeddingProvider>>()
        .ok_or_else(|| {
            ApiError(YorishiroError::Internal(anyhow::anyhow!(
                "EmbeddingProvider missing"
            )))
        })
}

/// The embedding provider `workspace_id` should actually use: its own assignment through the `WorkspaceEmbeddingResolver` seam if it has one, the deployment default otherwise.
/// Also used by `services::mcp`.
pub(crate) async fn resolve_embedding_provider(
    ctx: &AppContext,
    workspace_id: Uuid,
) -> Result<Arc<dyn crate::services::embedding::EmbeddingProvider>, ApiError> {
    let resolver = ctx
        .shared_store
        .get::<Arc<dyn crate::services::embedding::WorkspaceEmbeddingResolver>>()
        .ok_or_else(|| {
            ApiError(YorishiroError::Internal(anyhow::anyhow!(
                "WorkspaceEmbeddingResolver missing"
            )))
        })?;

    match resolver
        .resolve(&ctx.db, workspace_id)
        .await
        .map_err(ApiError)?
    {
        Some(provider) => Ok(provider),
        None => embedding_provider(ctx),
    }
}

/// The `WorkerClass` `workspace_id`'s queued jobs should carry: its own assignment through the `WorkerClassResolver` seam if it has one, `WorkerClass::Shared` otherwise.
pub(crate) async fn resolve_worker_class(
    ctx: &AppContext,
    workspace_id: Uuid,
) -> Result<crate::workers::embedding_sync::WorkerClass, ApiError> {
    let resolver = ctx
        .shared_store
        .get::<Arc<dyn crate::workers::embedding_sync::WorkerClassResolver>>()
        .ok_or_else(|| {
            ApiError(YorishiroError::Internal(anyhow::anyhow!(
                "WorkerClassResolver missing"
            )))
        })?;

    Ok(resolver
        .resolve(&ctx.db, workspace_id)
        .await
        .map_err(ApiError)?
        .unwrap_or(crate::workers::embedding_sync::WorkerClass::Shared))
}

/// See `db_handle`'s doc comment: also used by `services::mcp`.
pub(crate) fn search_token_limiter(
    ctx: &AppContext,
) -> Result<Arc<crate::services::rate_limit::RateLimiter>, ApiError> {
    ctx.shared_store
        .get::<Arc<crate::services::rate_limit::RateLimiter>>()
        .ok_or_else(|| {
            ApiError(YorishiroError::Internal(anyhow::anyhow!(
                "search token RateLimiter missing"
            )))
        })
}

/// The sole entry point for authenticated requests with no scope requirement and no DB connection.
/// Requiring this type as a handler argument is itself a declaration that "this route requires authentication."
pub struct AuthContext(pub auth::AuthContext);

impl<S> FromRequestParts<S> for AuthContext
where
    AppContext: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let presented_key = extract_bearer_key(parts)?;

        let app_ctx = AppContext::from_ref(state);

        // No DbHandle/Authenticator is built for SQLite (Hooks::after_context): that backend has no RLS to scope a request connection for and no ee/ authentication rule to abstract over, so this authenticates directly against ctx.db instead of going through the Authenticator seam.
        if app_ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
            let ctx = auth::authenticate_sqlite(&app_ctx.db, presented_key)
                .await
                .inspect_err(|err| log_auth_rejection(parts, err))?;
            auth::touch_last_used_sqlite(&app_ctx.db, ctx.api_key_id).await;
            return Ok(AuthContext(ctx));
        }

        let headers = header_pairs(parts);
        let db = db_handle(&app_ctx)?;
        let auth_impl = authenticator(&app_ctx)?;

        let ctx = auth_impl
            .authenticate(&db, presented_key, &headers)
            .await
            .inspect_err(|err| log_auth_rejection(parts, err))?;

        auth::touch_last_used_on(&db, ctx.tenant_id, ctx.workspace_id, ctx.api_key_id).await;

        Ok(AuthContext(ctx))
    }
}

/// Marker for declaring an endpoint's required API key scope at the type level.
pub trait RequiredScope {
    const SCOPE: ApiKeyScope;
}

pub struct ReadScope;
impl RequiredScope for ReadScope {
    const SCOPE: ApiKeyScope = ApiKeyScope::Read;
}

pub struct WriteScope;
impl RequiredScope for WriteScope {
    const SCOPE: ApiKeyScope = ApiKeyScope::Write;
}

pub struct SchemaScope;
impl RequiredScope for SchemaScope {
    const SCOPE: ApiKeyScope = ApiKeyScope::Schema;
}

pub struct MigrationScope;
impl RequiredScope for MigrationScope {
    const SCOPE: ApiKeyScope = ApiKeyScope::Migration;
}

/// An extractor that authenticates, verifies the required scope, and begins a transaction with the RLS context already set, all in one step (see `TenantDb::begin_for_workspace`).
/// There is no way to obtain a `DatabaseTransaction` on the tenant pool except through this type, which structurally prevents forgetting the scope check.
///
/// **A write handler must call `.commit().await?` before returning, or every write it made is silently discarded** (`DatabaseTransaction` rolls back on drop).
/// A read-only handler can just let it drop; a rollback of nothing-written is a no-op.
pub struct Authorized<R> {
    pub ctx: auth::AuthContext,
    txn: sea_orm::DatabaseTransaction,
    _scope: PhantomData<R>,
}

impl<R> Authorized<R> {
    pub fn txn(&self) -> &sea_orm::DatabaseTransaction {
        &self.txn
    }

    /// Commits the transaction.
    /// Every write handler must call this before returning `Ok`.
    pub async fn commit(self) -> Result<(), ApiError> {
        self.txn
            .commit()
            .await
            .map_err(|e| ApiError(YorishiroError::Internal(anyhow::anyhow!(e))))
    }
}

impl<S, R> FromRequestParts<S> for Authorized<R>
where
    AppContext: FromRef<S>,
    S: Send + Sync,
    R: RequiredScope,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let presented_key = extract_bearer_key(parts)?;

        let app_ctx = AppContext::from_ref(state);

        // See AuthContext's own SQLite branch: no DbHandle/Authenticator is built for that backend, so this authenticates and begins a plain transaction on ctx.db directly (auth::authorize_sqlite).
        if app_ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
            let (ctx, txn) = auth::authorize_sqlite(&app_ctx.db, presented_key, R::SCOPE)
                .await
                .inspect_err(|err| log_auth_rejection(parts, err))?;
            return Ok(Authorized {
                ctx,
                txn,
                _scope: PhantomData,
            });
        }

        let headers = header_pairs(parts);
        let db = db_handle(&app_ctx)?;
        let auth_impl = authenticator(&app_ctx)?;

        let (ctx, txn) =
            auth::authorize(&db, auth_impl.as_ref(), presented_key, R::SCOPE, &headers)
                .await
                .inspect_err(|err| log_auth_rejection(parts, err))?;

        Ok(Authorized {
            ctx,
            txn,
            _scope: PhantomData,
        })
    }
}

/// As `Authorized<R>`, but for the `audit` grant rather than a `RequiredScope`.
/// Not generic over `R` the way `Authorized<R>` is: `audit` is one grant, not a family of scopes, so there is nothing for a type parameter to select between.
pub struct AuditAuthorized {
    pub ctx: auth::AuthContext,
    txn: sea_orm::DatabaseTransaction,
}

impl AuditAuthorized {
    pub fn txn(&self) -> &sea_orm::DatabaseTransaction {
        &self.txn
    }
}

impl<S> FromRequestParts<S> for AuditAuthorized
where
    AppContext: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let presented_key = extract_bearer_key(parts)?;

        let app_ctx = AppContext::from_ref(state);

        // See Authorized<R>'s own SQLite branch.
        if app_ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
            let (ctx, txn) = auth::authorize_audit_sqlite(&app_ctx.db, presented_key)
                .await
                .inspect_err(|err| log_auth_rejection(parts, err))?;
            return Ok(AuditAuthorized { ctx, txn });
        }

        let headers = header_pairs(parts);
        let db = db_handle(&app_ctx)?;
        let auth_impl = authenticator(&app_ctx)?;

        let (ctx, txn) = auth::authorize_audit(&db, auth_impl.as_ref(), presented_key, &headers)
            .await
            .inspect_err(|err| log_auth_rejection(parts, err))?;

        Ok(AuditAuthorized { ctx, txn })
    }
}

/// A connection-less version of `Authorized<R>`: it only authenticates and verifies `R`'s scope, without acquiring a DB connection.
/// Handlers that do slow work before touching the database should use this instead and call `TenantDb::acquire_for_workspace` afterward.
///
/// **Deliberately has no SQLite branch, unlike `Authorized<R>`/`AuditAuthorized`.**
/// Its one caller, `search_entities` (`controllers/search.rs`), calls `db_handle(&ctx)?` directly in the handler body once the slow embedding call returns, so a SQLite branch here would still dead-end at that unconditional `db_handle` call.
/// The route is unreachable on SQLite for an independent reason regardless: `content_entities.embedding` (the pgvector column `search_by_vector` reads) does not exist on that backend at all (see `docs/sqlite.md`).
/// A future second caller of `Verified<R>` that doesn't touch `embedding`/`DbHandle` afterward would need this reconsidered.
pub struct Verified<R> {
    pub ctx: auth::AuthContext,
    _scope: PhantomData<R>,
}

impl<S, R> FromRequestParts<S> for Verified<R>
where
    AppContext: FromRef<S>,
    S: Send + Sync,
    R: RequiredScope,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let presented_key = extract_bearer_key(parts)?;
        let headers = header_pairs(parts);

        let app_ctx = AppContext::from_ref(state);
        let db = db_handle(&app_ctx)?;
        let auth_impl = authenticator(&app_ctx)?;

        let ctx = auth::authorize_scope(&db, auth_impl.as_ref(), presented_key, R::SCOPE, &headers)
            .await
            .inspect_err(|err| log_auth_rejection(parts, err))?;

        Ok(Verified {
            ctx,
            _scope: PhantomData,
        })
    }
}
