//! Request extractors that authenticate a caller and declare the required API key scope in the handler's type.
//!
//! `Authorized` also opens the RLS-scoped transaction, so a handler cannot reach tenant data without passing through it.

use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRef, FromRequestParts};
use axum::http::request::Parts;
use loco_rs::app::AppContext;

use crate::controllers::middleware::auth;
use crate::error::YorishiroError;
use crate::models::api_keys::{self, ApiKeyScope};

use super::ApiError;

/// Emits a `warn` for a request rejected before it reaches a handler (bad/missing key, or insufficient scope).
/// The presented key is never logged: only the caller IP (when `ConnectInfo` is present), the path, and the reason.
fn log_auth_rejection(parts: &Parts, err: &YorishiroError) {
    let client = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map_or_else(
            || "unknown".to_string(),
            |ConnectInfo(addr)| addr.ip().to_string(),
        );
    tracing::warn!(client = %client, path = %parts.uri.path(), error = %err, "request rejected during authentication");
}

/// Requires a nonempty bearer credential and logs rejection without exposing it.
fn extract_bearer_key(parts: &Parts) -> Result<&str, ApiError> {
    auth::extract_bearer_key(&parts.headers).ok_or_else(|| {
        let err = YorishiroError::Unauthenticated;
        log_auth_rejection(parts, &err);
        ApiError(err)
    })
}

/// The workspace search quota shared by REST and MCP.
pub(crate) fn search_token_limiter(
    ctx: &AppContext,
) -> Result<Arc<crate::controllers::middleware::rate_limit::RateLimiter>, ApiError> {
    ctx.shared_store
        .get::<Arc<crate::controllers::middleware::rate_limit::RateLimiter>>()
        .ok_or_else(|| {
            ApiError(YorishiroError::Internal(
                "search token RateLimiter missing".into(),
            ))
        })
}

/// The sole entry point for authenticated requests with no scope requirement and no DB connection.
///
/// Requiring this type as a handler argument is itself a declaration that "this route requires authentication."
pub struct AuthContext(pub api_keys::AuthContext);

impl<S> FromRequestParts<S> for AuthContext
where
    AppContext: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let presented_key = extract_bearer_key(parts)?;

        let app_ctx = AppContext::from_ref(state);

        let headers = auth::header_pairs(&parts.headers).map_err(ApiError)?;
        let ctx = auth::RequestAuth::new(&app_ctx)
            .authenticate(presented_key, &headers)
            .await
            .inspect_err(|err| log_auth_rejection(parts, err))?;

        Ok(Self(ctx))
    }
}

/// Marker for declaring an endpoint's required API key scope at the type level.
pub(crate) trait RequiredScope {
    const SCOPE: ApiKeyScope;
}

pub struct ReadScope;
impl RequiredScope for ReadScope {
    const SCOPE: ApiKeyScope = ApiKeyScope::Read;
}

pub(crate) struct WriteScope;
impl RequiredScope for WriteScope {
    const SCOPE: ApiKeyScope = ApiKeyScope::Write;
}

pub struct SchemaScope;
impl RequiredScope for SchemaScope {
    const SCOPE: ApiKeyScope = ApiKeyScope::Schema;
}

pub(crate) struct MigrationScope;
impl RequiredScope for MigrationScope {
    const SCOPE: ApiKeyScope = ApiKeyScope::Migration;
}

/// As `Authorized<R>`, but for the `audit` grant rather than a `RequiredScope`.
/// Not generic over `R` the way `Authorized<R>` is: `audit` is one grant, not a family of scopes, so there is nothing for a type parameter to select between.
pub(crate) struct AuditAuthorized {
    app_ctx: AppContext,
    pub ctx: api_keys::AuthContext,
}

impl AuditAuthorized {
    pub(crate) async fn records(
        &self,
        page: crate::models::pagination::ListParams,
    ) -> Result<Vec<crate::models::_entities::api_key_audit_log::Model>, ApiError> {
        crate::models::api_key_audit_log::list_for_actor(&self.app_ctx, &self.ctx, page)
            .await
            .map_err(ApiError)
    }

    pub(crate) fn timezone(&self) -> chrono_tz::Tz {
        crate::data::time::configured(&self.app_ctx)
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

        let headers = auth::header_pairs(&parts.headers).map_err(ApiError)?;
        let ctx = auth::RequestAuth::new(&app_ctx)
            .verify_audit(presented_key, &headers)
            .await
            .inspect_err(|err| log_auth_rejection(parts, err))?;
        Ok(Self { app_ctx, ctx })
    }
}

/// A connection-less version of `Authorized<R>`: it only authenticates and verifies `R`'s scope, without acquiring a DB connection.
///
/// Handlers that do slow work before touching the database should use this instead and call `TenantDb::acquire_for_workspace` afterward.
pub struct Verified<R> {
    pub ctx: api_keys::AuthContext,
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
        let headers = auth::header_pairs(&parts.headers).map_err(ApiError)?;

        let app_ctx = AppContext::from_ref(state);

        let ctx = auth::RequestAuth::new(&app_ctx)
            .verify(presented_key, R::SCOPE, &headers)
            .await
            .inspect_err(|err| log_auth_rejection(parts, err))?;

        Ok(Self {
            ctx,
            _scope: PhantomData,
        })
    }
}

/// Documents what every authenticating extractor does: the bearer scheme and, where the type names one, the scope.
pub(crate) fn document_authentication(
    _: &mut aide::generate::GenContext,
    operation: &mut aide::openapi::Operation,
    scope: Option<ApiKeyScope>,
) {
    let requirement = std::iter::once(("bearer_auth".to_owned(), Vec::new())).collect();
    if !operation.security.contains(&requirement) {
        operation.security.push(requirement);
    }
    if let Some(scope) = scope {
        operation.extensions.insert(
            "x-yorishiro-required-scopes".into(),
            serde_json::json!([scope.as_db_str()]),
        );
    }
}

impl aide::OperationInput for AuthContext {
    fn operation_input(
        ctx: &mut aide::generate::GenContext,
        operation: &mut aide::openapi::Operation,
    ) {
        document_authentication(ctx, operation, None);
    }
}

impl aide::OperationInput for AuditAuthorized {
    fn operation_input(
        ctx: &mut aide::generate::GenContext,
        operation: &mut aide::openapi::Operation,
    ) {
        document_authentication(ctx, operation, None);
    }
}

impl<R: RequiredScope> aide::OperationInput for Verified<R> {
    fn operation_input(
        ctx: &mut aide::generate::GenContext,
        operation: &mut aide::openapi::Operation,
    ) {
        document_authentication(ctx, operation, Some(R::SCOPE));
    }
}
