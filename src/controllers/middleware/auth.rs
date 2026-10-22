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
        db: &DbHandle,
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
        db: &DbHandle,
        presented_key: &str,
        _headers: &[(String, String)],
    ) -> Result<AuthContext, YorishiroError> {
        Entity::authenticate(db, presented_key).await
    }
}

/// The authenticator a deployment gets when it does not choose one.
pub fn default_authenticator() -> Arc<dyn Authenticator> {
    Arc::new(DefaultAuthenticator)
}

/// Enforces that an authenticated context satisfies the required scope, returning
/// `YorishiroError::ScopeInsufficient` when it doesn't.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub fn require_scope(ctx: &AuthContext, required: ApiKeyScope) -> Result<(), YorishiroError> {
    if ctx.scope.satisfies(required) {
        Ok(())
    } else {
        Err(YorishiroError::ScopeInsufficient {
            message: format!(
                "this operation requires {required:?} scope but the API key has {:?} scope",
                ctx.scope
            ),
            hint: "Reissue an API key with sufficient scope".into(),
        })
    }
}

/// Enforces that an authenticated context holds the independent `audit` grant.
/// Deliberately not folded into `require_scope`/`ApiKeyScope::satisfies`: `audit` is not one more rung above `Migration` on the read/write/schema/migration ladder, so it is never compared with `Ord`, only checked for `true`/`false`.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub fn require_audit(ctx: &AuthContext) -> Result<(), YorishiroError> {
    if ctx.audit {
        Ok(())
    } else {
        Err(YorishiroError::ScopeInsufficient {
            message: "this operation requires the audit grant, which this API key does not hold"
                .into(),
            hint: "reissue an API key with audit:true".into(),
        })
    }
}

/// Request authentication shared by transport adapters, with backend details kept here.
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
        if self.app.is_sqlite() {
            Entity::authenticate_sqlite(&self.app.db, key).await
        } else {
            let db = self.db()?;
            let authenticator = self
                .app
                .shared_store
                .get::<Arc<dyn Authenticator>>()
                .ok_or_else(|| {
                    YorishiroError::Internal(anyhow::anyhow!("Authenticator missing"))
                })?;
            authenticator.authenticate(&db, key, headers).await
        }
    }

    fn db(&self) -> Result<DbHandle, YorishiroError> {
        self.app
            .shared_store
            .get::<DbHandle>()
            .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))
    }

    async fn touch(&self, ctx: &AuthContext) -> Result<(), YorishiroError> {
        if self.app.is_sqlite() {
            Entity::touch_last_used(&self.app.db, ctx.api_key_id).await;
        } else {
            touch_last_used_on(&self.db()?, ctx.tenant_id, ctx.workspace_id, ctx.api_key_id).await;
        }
        Ok(())
    }

    pub(crate) async fn authenticate(
        &self,
        key: &str,
        headers: &[(String, String)],
    ) -> Result<AuthContext, YorishiroError> {
        let ctx = self.resolve(key, headers).await?;
        self.touch(&ctx).await?;
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
        self.touch(&ctx).await?;
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
        self.touch(&ctx).await?;
        let txn = if write {
            crate::db::begin_workspace_write(self.app, ctx.tenant_id, ctx.workspace_id).await?
        } else {
            crate::db::begin_workspace(self.app, ctx.tenant_id, ctx.workspace_id).await?
        };
        Ok((ctx, txn))
    }
}

/// Touches `last_used_at` through a connection freshly acquired for this workspace, logging (never failing the caller) on either call's error.
pub(crate) async fn touch_last_used_on(
    db: &DbHandle,
    tenant_id: uuid::Uuid,
    workspace_id: uuid::Uuid,
    api_key_id: uuid::Uuid,
) {
    match db
        .tenant
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
    {
        Ok(mut conn) => {
            if let Err(err) = Entity::touch_last_used_on_connection(conn.as_mut(), api_key_id).await
            {
                tracing::warn!(error = %err, "failed to update api key last_used_at");
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to acquire connection to touch last_used_at");
        }
    }
}
