//! The authentication guard every entry point (REST, MCP) resolves a request through.
//!
//! Persistence of keys lives on `models::api_keys`; this module turns a presented credential into an authorized, tenant-scoped transaction.
//! [`Authenticator`] is the seam an edition replaces, so a new authenticated entry point must go through it rather than calling `Entity::authenticate` directly.

use std::sync::Arc;

use axum::http::request::Parts;
use loco_rs::app::AppContext;
use loco_rs::prelude::async_trait;
use sea_orm::DatabaseTransaction;

use crate::db::{AppContextBackend, DbHandle};
use crate::error::YorishiroError;
use crate::models::api_keys::{ApiKeyScope, AuthContext, Entity};

/// Copies readable request headers into the authenticator input shape.
pub(crate) fn header_pairs(parts: &Parts) -> Vec<(String, String)> {
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

/// Extracts the bearer credential from an Authorization header.
pub(crate) fn extract_bearer_key(parts: &Parts) -> Option<&str> {
    bearer_credential(
        parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
    )
}

/// Extracts the API key from an `Authorization` header value, or `None` if the header is absent, is not a `Bearer` credential, or carries an empty one.
///
/// Every adapter that authenticates a request routes through here, so `Authorization: Bearer ` with nothing after it gets the same answer everywhere.
pub fn bearer_credential(header_value: Option<&str>) -> Option<&str> {
    header_value
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
}

/// Resolves a presented API key into an [`AuthContext`].
///
/// A seam: a deployment can replace this rule (a key that names its workspace per request, a key issued by an external identity system) without touching the routes that call it.
/// `DefaultAuthenticator` is the behaviour of every deployment that does not replace it.
///
/// # Contract
///
/// - **must** reject a key it cannot verify, by returning [`YorishiroError::Unauthenticated`].
/// - **must** return a context whose `tenant_id` owns its `workspace_id`.
///   The RLS session variables are set from both, so a mismatched pair silently produces a session that can see one tenant's workspace under another tenant's policies.
/// - **may** read `headers` for anything the key itself does not carry.
#[async_trait]
pub trait Authenticator: Send + Sync {
    async fn authenticate(
        &self,
        app: &AppContext,
        presented_key: &str,
        headers: &[(String, String)],
    ) -> Result<AuthContext, YorishiroError>;
}

/// This crate's own rule: a key is bound to exactly one workspace, recorded on the key itself, and the request's headers do not affect which one it resolves to.
pub(crate) struct DefaultAuthenticator;

#[async_trait]
impl Authenticator for DefaultAuthenticator {
    async fn authenticate(
        &self,
        app: &AppContext,
        presented_key: &str,
        _headers: &[(String, String)],
    ) -> Result<AuthContext, YorishiroError> {
        if app.is_sqlite() {
            Entity::authenticate_sqlite(&app.db, presented_key).await
        } else {
            let db = app
                .shared_store
                .get::<DbHandle>()
                .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
            Entity::authenticate(&db, presented_key).await
        }
    }
}

/// The authenticator a deployment gets when it does not choose one.
pub fn default_authenticator() -> Arc<dyn Authenticator> {
    Arc::new(DefaultAuthenticator)
}

pub use crate::models::access::{require_audit, require_scope};

/// Request authentication shared by transport adapters through the installed policy.
pub(crate) struct RequestAuth<'a> {
    app: &'a AppContext,
}

impl<'a> RequestAuth<'a> {
    pub(crate) fn new(app: &'a AppContext) -> Self {
        Self { app }
    }

    pub(crate) async fn resolve(
        &self,
        key: &str,
        headers: &[(String, String)],
    ) -> Result<AuthContext, YorishiroError> {
        let authenticator = self
            .app
            .shared_store
            .get::<Arc<dyn Authenticator>>()
            .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("Authenticator missing")))?;
        authenticator.authenticate(self.app, key, headers).await
    }

    pub(crate) async fn authenticate(
        &self,
        key: &str,
        headers: &[(String, String)],
    ) -> Result<AuthContext, YorishiroError> {
        let ctx = self.resolve(key, headers).await?;
        Entity::touch_authenticated(self.app, &ctx).await?;
        Ok(ctx)
    }

    pub(crate) async fn verify(
        &self,
        key: &str,
        required: ApiKeyScope,
        headers: &[(String, String)],
    ) -> Result<AuthContext, YorishiroError> {
        let ctx = self.resolve(key, headers).await?;
        require_scope(&ctx, required)?;
        Entity::touch_authenticated(self.app, &ctx).await?;
        Ok(ctx)
    }

    pub(crate) async fn authorize(
        &self,
        key: &str,
        required: ApiKeyScope,
        headers: &[(String, String)],
    ) -> Result<(AuthContext, DatabaseTransaction), YorishiroError> {
        let ctx = self.resolve(key, headers).await?;
        require_scope(&ctx, required)?;
        self.begin(ctx, required >= ApiKeyScope::Write).await
    }

    pub(crate) async fn authorize_audit(
        &self,
        key: &str,
        headers: &[(String, String)],
    ) -> Result<(AuthContext, DatabaseTransaction), YorishiroError> {
        let ctx = self.resolve(key, headers).await?;
        require_audit(&ctx)?;
        self.begin(ctx, false).await
    }

    async fn begin(
        &self,
        ctx: AuthContext,
        write: bool,
    ) -> Result<(AuthContext, DatabaseTransaction), YorishiroError> {
        // Touch independently: read handlers roll their workspace transaction back on drop.
        Entity::touch_authenticated(self.app, &ctx).await?;
        let txn = if write {
            crate::db::begin_workspace_write(self.app, ctx.tenant_id, ctx.workspace_id).await?
        } else {
            crate::db::begin_workspace(self.app, ctx.tenant_id, ctx.workspace_id).await?
        };
        Ok((ctx, txn))
    }
}
