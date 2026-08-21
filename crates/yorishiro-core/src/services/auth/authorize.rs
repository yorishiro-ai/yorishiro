use sqlx::Postgres;
use sqlx::pool::PoolConnection;

use crate::db::DbHandle;
use crate::error::{ResultExt, YorishiroError};

use super::{ApiKeyScope, AuthContext, Authenticator, touch_last_used};

/// Enforces that an authenticated context satisfies the required scope, returning `YorishiroError::ScopeInsufficient` when it doesn't.
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

/// The single entry point for authorization: validates the presented raw key, confirms it satisfies the required scope, and returns a connection with the RLS context already set.
/// REST and MCP adapters have no way to obtain a `&mut PgConnection` except through this function, which structurally prevents a scope check from being forgotten.
/// The key is resolved through `authenticator` rather than by this function directly, so a deployment that replaces the rule (see [`Authenticator`]) is honoured on every path that authorizes (REST and MCP alike) rather than only where it remembered to look.
/// `headers` is passed through untouched for an implementation that reads them.
///
/// Postgres only for now: the data-plane handlers this feeds are still Postgres-concrete, so the Sqlite arm fails loudly rather than pretending to hand out a connection type it cannot produce.
/// [`authorize_scope`] below has no such limit.
pub async fn authorize(
    db: &DbHandle,
    authenticator: &dyn Authenticator,
    presented_key: &str,
    required: ApiKeyScope,
    headers: &[(String, String)],
) -> Result<(AuthContext, PoolConnection<Postgres>), YorishiroError> {
    let tenant = match db {
        DbHandle::Postgres { tenant, .. } => tenant,
        #[cfg(feature = "sqlite")]
        DbHandle::Sqlite(_) => {
            return Err(YorishiroError::Internal(anyhow::anyhow!(
                "authorize (a Postgres connection) is not available on the Sqlite engine yet"
            )));
        }
    };

    let ctx = authenticator
        .authenticate(db, presented_key, headers)
        .await?;
    require_scope(&ctx, required)?;

    let mut conn = tenant
        .acquire_for_workspace(ctx.tenant_id, ctx.workspace_id)
        .await
        .internal()?;

    if let Err(err) = touch_last_used(conn.as_mut(), ctx.api_key_id).await {
        tracing::warn!(error = %err, "failed to update api key last_used_at");
    }

    Ok((ctx, conn))
}

/// Touches `last_used_at` through a connection freshly acquired on whichever engine `db` runs, logging (never failing the caller) on either step's error.
///
/// The one place that knows how to reach a connection for this best-effort update on both engines; every other seam that needs the same update (the `AuthContext` extractor, `authorize_scope` below) calls this instead of matching on `db` itself.
pub async fn touch_last_used_on(
    db: &DbHandle,
    tenant_id: uuid::Uuid,
    workspace_id: uuid::Uuid,
    api_key_id: uuid::Uuid,
) {
    match db {
        DbHandle::Postgres { tenant, .. } => {
            match tenant.acquire_for_workspace(tenant_id, workspace_id).await {
                Ok(mut conn) => {
                    if let Err(err) = touch_last_used(conn.as_mut(), api_key_id).await {
                        tracing::warn!(error = %err, "failed to update api key last_used_at");
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed to acquire connection to touch last_used_at");
                }
            }
        }
        #[cfg(feature = "sqlite")]
        DbHandle::Sqlite(sqlite) => {
            match crate::db::Storage::acquire_for_workspace(sqlite, tenant_id, workspace_id).await {
                Ok(mut conn) => {
                    if let Err(err) = touch_last_used(conn.as_mut(), api_key_id).await {
                        tracing::warn!(error = %err, "failed to update api key last_used_at");
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed to acquire connection to touch last_used_at");
                }
            }
        }
    }
}

/// A connection-free variant of `authorize`, used on paths (search queries) that need to run a slow step (like embedding generation) before touching the DB.
/// `authorize` holds a connection for the handler's entire lifetime, which would tie up a pool connection during embedding generation (unbounded wait time with LocalOnnx due to in-process serialization), letting pool exhaustion spill over onto unrelated endpoints.
/// This function only performs authentication and scope validation, updating `last_used_at` through a short-lived connection that's returned immediately.
///
/// Works on both engines: unlike `authorize`, nothing here hands the caller a connection whose type would have to name one engine.
pub async fn authorize_scope(
    db: &DbHandle,
    authenticator: &dyn Authenticator,
    presented_key: &str,
    required: ApiKeyScope,
    headers: &[(String, String)],
) -> Result<AuthContext, YorishiroError> {
    let ctx = authenticator
        .authenticate(db, presented_key, headers)
        .await?;
    require_scope(&ctx, required)?;

    touch_last_used_on(db, ctx.tenant_id, ctx.workspace_id, ctx.api_key_id).await;

    Ok(ctx)
}

#[cfg(test)]
#[path = "../../../tests/services/auth/authorize.rs"]
mod tests;
