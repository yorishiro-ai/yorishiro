use uuid::Uuid;

use crate::db::DbHandle;
use crate::error::{ResultExt, YorishiroError};

use super::{ApiKeyScope, AuthContext, hash_key};

/// Verifies a presented raw API key and resolves the workspace, tenant, and scope it belongs
/// to.
///
/// At this point neither the workspace nor the tenant is known yet, so RLS's
/// `app.current_workspace`/`app.current_tenant` can't be set, which is why this takes the
/// whole [`DbHandle`] rather than a scoped connection.
pub async fn authenticate(
    db: &DbHandle,
    presented_key: &str,
) -> Result<AuthContext, YorishiroError> {
    let key_hash = hash_key(presented_key);

    // `authenticate_api_key` is SECURITY DEFINER so this bypasses RLS on
    // identity_api_keys/identity_workspaces for verification purposes only, and limits the
    // columns it returns to id/workspace_id/tenant_id/scope/user_id (never key_hash itself).
    // Runs on the tenant pool, not identity's: the migration's REVOKE ALL ... GRANT EXECUTE
    // grants this function to the role every request already connects as (yorishiro_app).
    let row: Option<(Uuid, Uuid, Uuid, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, workspace_id, tenant_id, scope, user_id FROM authenticate_api_key($1)",
    )
    .bind(key_hash)
    .fetch_optional(db.tenant.pool())
    .await
    .internal()?;

    let (api_key_id, workspace_id, tenant_id, scope_str, user_id) =
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
    })
}

/// Records the API key's last-used timestamp, on a raw `sqlx` connection.
/// Best-effort: doesn't affect authentication outcomes, so callers don't need to fail the
/// whole request if it errors.
///
/// Deliberately not run on the request's `DatabaseTransaction`: a read-only handler drops that
/// transaction without committing (see `Authorized::commit`'s doc comment), which would
/// silently roll this update back along with it. Every caller uses a short-lived connection
/// from `TenantDb::acquire_for_workspace` instead (see `authorize`/`touch_last_used_on`), which
/// persists independently of whatever the request transaction does.
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
