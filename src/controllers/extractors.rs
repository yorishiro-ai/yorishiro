use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRef, FromRequestParts};
use axum::http::header;
use axum::http::request::Parts;
use loco_rs::app::AppContext;

use crate::db::DbHandle;
use crate::error::YorishiroError;
use crate::services::auth;
use crate::services::auth::{ApiKeyScope, Authenticator};

use super::ApiError;

/// Emits a `warn` for a request rejected before it reaches a handler (bad/missing key, or
/// insufficient scope). The presented key is never logged: only the caller IP (when
/// `ConnectInfo` is present), the path, and the reason.
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

/// Also used by the MCP adapter (`services::mcp`), which authorizes per-tool rather than through
/// this file's `FromRequestParts` impls, but still needs the same `DbHandle` out of
/// `shared_store`.
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

/// The sole entry point for authenticated requests with no scope requirement and no DB
/// connection. Requiring this type as a handler argument is itself a declaration that "this
/// route requires authentication."
pub struct AuthContext(pub auth::AuthContext);

impl<S> FromRequestParts<S> for AuthContext
where
    AppContext: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let presented_key = extract_bearer_key(parts)?;
        let headers = header_pairs(parts);

        let app_ctx = AppContext::from_ref(state);
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

/// An extractor that authenticates, verifies the required scope, and begins a transaction with
/// the RLS context already set, all in one step (see `TenantDb::begin_for_workspace`). There is
/// no way to obtain a `DatabaseTransaction` on the tenant pool except through this type, which
/// structurally prevents forgetting the scope check.
///
/// **A write handler must call `.commit().await?` before returning, or every write it made is
/// silently discarded** (`DatabaseTransaction` rolls back on drop). A read-only handler can just
/// let it drop; a rollback of nothing-written is a no-op.
pub struct Authorized<R> {
    pub ctx: auth::AuthContext,
    txn: sea_orm::DatabaseTransaction,
    _scope: PhantomData<R>,
}

impl<R> Authorized<R> {
    pub fn txn(&self) -> &sea_orm::DatabaseTransaction {
        &self.txn
    }

    /// Commits the transaction. Every write handler must call this before returning `Ok`.
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
        let headers = header_pairs(parts);

        let app_ctx = AppContext::from_ref(state);
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

/// A connection-less version of `Authorized<R>`: it only authenticates and verifies `R`'s
/// scope, without acquiring a DB connection. Handlers that do slow work before touching the
/// database should use this instead and call `TenantDb::acquire_for_workspace` afterward.
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
