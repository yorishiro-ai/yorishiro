//! Serializes reindexes of one workspace across processes and runs one under that lock.
//!
//! PostgreSQL takes a session-scoped advisory lock on a detached connection; SQLite relies on the model's transactional writes.

use sea_orm::sqlx::{Connection, PgPool};
use uuid::Uuid;

/// A guard that holds a session-scoped advisory lock on a workspace for reindex serialization.
///
/// `conn` is detached so returning it to the pool does not release the lock.
/// `release().await` closes the connection asynchronously, sends PostgreSQL the `Terminate`
/// message, ends the session, and releases the lock.
///
/// `Drop` is only a panic-unwind backstop: it tries to spawn `conn.close()` on the current
/// runtime, which may or may not succeed depending on the runtime state, but is better
/// than leaving the connection open with no cleanup attempt at all.
pub struct WorkspaceReindexLockGuard {
    conn: Option<sea_orm::sqlx::PgConnection>,
}

impl WorkspaceReindexLockGuard {
    /// Closes the detached connection and releases the advisory lock.
    ///
    /// Returns `Ok(())` on success. If the connection was already released
    /// (double-`release` is a no-op), returns `Ok(())` without error.
    /// Also logs any close failure for diagnostics.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn release(mut self) -> Result<(), sea_orm::sqlx::Error> {
        if let Some(conn) = self.conn.take() {
            let result = conn.close().await;
            if let Err(ref e) = result {
                tracing::error!(error = %e, "failed to close reindex lock connection");
            }
            result
        } else {
            Ok(())
        }
    }
}

impl Drop for WorkspaceReindexLockGuard {
    fn drop(&mut self) {
        // This path runs only if the caller forgot to call `release()` — a
        // logic error.  Try to close the connection asynchronously; the
        // explicit `release().await` is the intended path.
        if let Some(conn) = self.conn.take()
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            handle.spawn(async move {
                let _ = conn.close().await;
            });
        }
        // If we cannot get a runtime handle (no current runtime, or a
        // different runtime), we have no way to close the connection
        // asynchronously. The client socket closes when the process exits.
    }
}

/// The advisory lock namespace for reindexes, separate from entity quota locks.
#[must_use]
pub fn workspace_reindex_lock_key(workspace_id: Uuid) -> String {
    format!("embedding-reindex:{workspace_id}")
}

/// Acquires a session-scoped advisory lock on a workspace for reindex serialization.
///
/// Acquires `pg_advisory_lock` on a bare pooled connection and detaches it. The lock is held
/// until `WorkspaceReindexLockGuard::release()` is called, which closes the connection and
/// ends the session. Pool return keeps the session alive, so the lock would persist and cause
/// the next reindex to re-enter (advisory locks are per-session, not per-connection).
///
/// Does not time out: a legitimate reindex over a large workspace can block for a long time,
/// and imposing a bound would risk killing a run that is just busy. The `tracing::info!`
/// before and after acquisition makes a wait visible rather than a silent hang.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn acquire_workspace_reindex_lock(
    pool: PgPool,
    workspace_id: Uuid,
) -> Result<WorkspaceReindexLockGuard, sea_orm::sqlx::Error> {
    let mut conn = pool.acquire().await?;

    // Reindexes serialize with each other, not with entity quota locks during provider calls.
    let key = workspace_reindex_lock_key(workspace_id);
    tracing::info!(%key, "waiting for workspace reindex lock");

    sea_orm::sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(&key)
        .execute(conn.as_mut())
        .await?;

    tracing::info!(%key, "acquired workspace reindex lock");

    // Detach the connection: returning it to the pool would keep the session (and the lock)
    // alive. `release()` on the guard closes the connection, ending the session.
    let detached = conn.detach();

    Ok(WorkspaceReindexLockGuard {
        conn: Some(detached),
    })
}

/// Runs a reindex with the backend's workspace serialization policy.
/// PostgreSQL holds a detached session lock; SQLite retains the model's transactional writes without a session lock.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn reindex_workspace_with_lock(
    ctx: &loco_rs::app::AppContext,
    workspace_id: Uuid,
    provider: &dyn crate::services::embedding::EmbeddingProvider,
) -> Result<crate::models::entity_embeddings::ReindexOutcome, crate::YorishiroError> {
    reindex_workspace_with_actor(ctx, workspace_id, provider, None, false).await
}

pub(crate) async fn reindex_workspace_with_actor(
    ctx: &loco_rs::app::AppContext,
    workspace_id: Uuid,
    provider: &dyn crate::services::embedding::EmbeddingProvider,
    actor: Option<&crate::models::access_keys::AuthContext>,
    startup: bool,
) -> Result<crate::models::entity_embeddings::ReindexOutcome, crate::YorishiroError> {
    let lock = if crate::db::is_sqlite(&ctx.db) {
        None
    } else {
        let handle = ctx
            .shared_store
            .get::<crate::db::DbHandle>()
            .ok_or_else(|| {
                crate::YorishiroError::Internal(
                    "reindex requires the tenant pool, which this deployment did not build".into(),
                )
            })?;
        Some(
            acquire_workspace_reindex_lock(handle.tenant.pool().clone(), workspace_id)
                .await
                .map_err(|err| {
                    crate::YorishiroError::Internal(
                        format!("failed to acquire workspace lock: {err}").into(),
                    )
                })?,
        )
    };
    let outcome = crate::models::entity_embeddings::reindex_workspace_as(
        &ctx.db,
        workspace_id,
        provider,
        actor,
        startup,
    )
    .await;
    if let Some(lock) = lock {
        let _ = lock.release().await;
    }
    outcome
}
