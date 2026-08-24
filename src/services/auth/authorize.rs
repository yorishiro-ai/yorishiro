use sea_orm::DatabaseTransaction;

use crate::db::DbHandle;
use crate::error::{ResultExt, YorishiroError};

use super::{ApiKeyScope, AuthContext, Authenticator, touch_last_used};

/// Enforces that an authenticated context satisfies the required scope, returning
/// `YorishiroError::ScopeInsufficient` when it doesn't.
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

/// The single entry point for authorization: validates the presented raw key, confirms it satisfies the required scope, and returns a transaction with the RLS context already set (see `TenantDb::begin_for_workspace`).
/// REST and MCP adapters have no other way to obtain a `DatabaseTransaction` on the tenant pool, so a scope check can't be forgotten.
///
/// The caller owns the returned transaction's lifetime: a write handler must call `txn.commit().await` explicitly, or every write in it is silently discarded when the transaction drops.
///
/// `last_used_at` is touched through `touch_last_used_on`'s independent short-lived connection, not on the returned transaction: a read-only handler drops its transaction without committing, which would silently roll the update back if it ran there.
pub async fn authorize(
    db: &DbHandle,
    authenticator: &dyn Authenticator,
    presented_key: &str,
    required: ApiKeyScope,
    headers: &[(String, String)],
) -> Result<(AuthContext, DatabaseTransaction), YorishiroError> {
    let ctx = authenticator
        .authenticate(db, presented_key, headers)
        .await?;
    require_scope(&ctx, required)?;

    let txn = db
        .tenant
        .begin_for_workspace(ctx.tenant_id, ctx.workspace_id)
        .await
        .internal()?;

    touch_last_used_on(db, ctx.tenant_id, ctx.workspace_id, ctx.api_key_id).await;

    Ok((ctx, txn))
}

/// As `authorize`, but checking `require_audit` instead of a scope.
/// Kept as its own function rather than a parameter on `authorize`, since the two checks are different shapes (`ApiKeyScope` vs. a bare grant) and forcing them through one signature would mean threading an `Option<ApiKeyScope>`/enum through every existing `authorize` call site for the sake of the one caller that needs this.
pub async fn authorize_audit(
    db: &DbHandle,
    authenticator: &dyn Authenticator,
    presented_key: &str,
    headers: &[(String, String)],
) -> Result<(AuthContext, DatabaseTransaction), YorishiroError> {
    let ctx = authenticator
        .authenticate(db, presented_key, headers)
        .await?;
    require_audit(&ctx)?;

    let txn = db
        .tenant
        .begin_for_workspace(ctx.tenant_id, ctx.workspace_id)
        .await
        .internal()?;

    touch_last_used_on(db, ctx.tenant_id, ctx.workspace_id, ctx.api_key_id).await;

    Ok((ctx, txn))
}

/// Touches `last_used_at` through a connection freshly acquired for this workspace, logging (never failing the caller) on either step's error.
pub async fn touch_last_used_on(
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
            if let Err(err) = touch_last_used(conn.as_mut(), api_key_id).await {
                tracing::warn!(error = %err, "failed to update api key last_used_at");
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to acquire connection to touch last_used_at");
        }
    }
}

/// A connection-free variant of `authorize`, used on paths that need to run a slow step (like embedding generation) before touching the DB: it only authenticates and validates scope, updating `last_used_at` through a short-lived connection that's returned immediately.
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
