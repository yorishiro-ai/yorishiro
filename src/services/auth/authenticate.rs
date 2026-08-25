use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::db::DbHandle;
use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{identity_api_keys, identity_workspaces};

use super::{ApiKeyScope, AuthContext, hash_key};

/// Verifies a presented raw API key and resolves the workspace, tenant, and scope it belongs to.
///
/// At this point neither the workspace nor the tenant is known yet, so RLS's `app.current_workspace`/`app.current_tenant` can't be set, which is why this takes the whole [`DbHandle`] rather than a scoped connection.
pub async fn authenticate(
    db: &DbHandle,
    presented_key: &str,
) -> Result<AuthContext, YorishiroError> {
    let key_hash = hash_key(presented_key);

    // `authenticate_api_key` is SECURITY DEFINER, so this bypasses RLS on identity_api_keys/identity_workspaces, and limits the columns it returns to id/workspace_id/tenant_id/scope/user_id/audit (never key_hash itself).
    let row: Option<(Uuid, Uuid, Uuid, String, Option<Uuid>, bool)> = sqlx::query_as(
        "SELECT id, workspace_id, tenant_id, scope, user_id, audit FROM authenticate_api_key($1)",
    )
    .bind(key_hash)
    .fetch_optional(db.tenant.pool())
    .await
    .internal()?;

    let (api_key_id, workspace_id, tenant_id, scope_str, user_id, audit) =
        row.ok_or(YorishiroError::Unauthenticated)?;
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

/// SQLite equivalent of [`authenticate`], for a deployment with no `DbHandle` (see `Hooks::after_context`).
///
/// `authenticate`'s Postgres path goes through the `authenticate_api_key` SECURITY DEFINER function specifically to read rows RLS would otherwise hide from an unauthenticated caller; SQLite has no RLS at all, so there is nothing to bypass, and this queries `identity_api_keys`/`identity_workspaces` directly with the SeaORM entity API.
/// Matches the SQL function's single-argument overload exactly: only a workspace-scoped key (`workspace_id` set) resolves, since the join is on `k.workspace_id`; a tenant-scoped key (`workspace_id` NULL) matches nothing here either, same as on Postgres.
///
/// Deliberately not routed through the `Authenticator` trait (`crate::services::auth::authenticator`): that seam exists so `ee/` can swap the authentication rule without touching call sites, and `ee/` does not run against SQLite (see `.claude/rules/loco-architecture.md`), so there is no second implementation for the seam to abstract over on this backend.
pub async fn authenticate_sqlite(
    conn: &impl ConnectionTrait,
    presented_key: &str,
) -> Result<AuthContext, YorishiroError> {
    let key_hash = hash_key(presented_key);

    let key = identity_api_keys::Entity::find()
        .filter(identity_api_keys::Column::KeyHash.eq(key_hash))
        .one(conn)
        .await
        .internal()?
        .ok_or(YorishiroError::Unauthenticated)?;

    let Some(workspace_id) = key.workspace_id else {
        return Err(YorishiroError::Unauthenticated);
    };

    let workspace = identity_workspaces::Entity::find_by_id(workspace_id)
        .one(conn)
        .await
        .internal()?
        .ok_or(YorishiroError::Unauthenticated)?;

    let scope = ApiKeyScope::from_db_str(&key.scope).ok_or_else(|| {
        YorishiroError::Internal(anyhow::anyhow!(
            "unknown api key scope in database: {}",
            key.scope
        ))
    })?;

    Ok(AuthContext {
        api_key_id: key.id,
        workspace_id,
        tenant_id: workspace.tenant_id,
        scope,
        user_id: key.user_id,
        audit: key.audit,
    })
}

/// SQLite equivalent of [`touch_last_used`]: same best-effort, non-failing update, on the SeaORM entity API instead of a raw `sqlx::PgConnection`.
pub async fn touch_last_used_sqlite(conn: &impl ConnectionTrait, api_key_id: Uuid) {
    let result = identity_api_keys::Entity::update_many()
        .col_expr(
            identity_api_keys::Column::LastUsedAt,
            sea_orm::sea_query::Expr::value(chrono::Utc::now()),
        )
        .filter(identity_api_keys::Column::Id.eq(api_key_id))
        .exec(conn)
        .await;
    if let Err(err) = result {
        tracing::warn!(error = %err, "failed to update api key last_used_at");
    }
}

/// Records the API key's last-used timestamp, on a raw `sqlx` connection.
/// Best-effort: doesn't affect authentication outcomes, so callers don't need to fail the whole request if it errors.
///
/// Deliberately not run on the request's `DatabaseTransaction`: a read-only handler drops that transaction without committing, which would silently roll this update back along with it.
/// Every caller uses a short-lived connection from `TenantDb::acquire_for_workspace` instead (see `authorize`/`touch_last_used_on`).
pub async fn touch_last_used(
    conn: &mut sqlx::PgConnection,
    api_key_id: Uuid,
) -> Result<(), YorishiroError> {
    sqlx::query("UPDATE identity_api_keys SET last_used_at = now() WHERE id = $1")
        .bind(api_key_id)
        .execute(conn)
        .await
        .internal()?;
    Ok(())
}
