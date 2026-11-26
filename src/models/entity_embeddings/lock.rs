//! Serializes reindexes of one workspace across processes and runs one under that lock.
//!
//! PostgreSQL takes a session-scoped advisory lock on a detached connection; SQLite relies on the model's transactional writes.

use sea_orm::sqlx::{Connection, PgPool};
use uuid::Uuid;

/// A guard that holds a session-scoped advisory lock on a workspace for reindex serialization.
///
/// `conn` is detached so returning it to the pool does not release the lock.
/// `release().await` closes the connection, which ends the session and releases the lock.
///
/// `Drop` is only a backstop: it spawns `conn.close()` on the current runtime when there is one.
pub struct WorkspaceReindexLockGuard {
    conn: Option<sea_orm::sqlx::PgConnection>,
}

impl WorkspaceReindexLockGuard {
    /// Closes the detached connection and releases the advisory lock.
    ///
    /// A close failure is logged and returned.
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
        // Reached only when the caller skipped `release()`.
        if let Some(conn) = self.conn.take()
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            handle.spawn(async move {
                let _ = conn.close().await;
            });
        }
    }
}

/// The advisory lock namespace for reindexes, separate from entity quota locks.
#[must_use]
pub fn workspace_reindex_lock_key(workspace_id: Uuid) -> String {
    format!("embedding-reindex:{workspace_id}")
}

/// Acquires a session-scoped advisory lock on a workspace for reindex serialization.
///
/// Takes `pg_advisory_lock` on a pooled connection and detaches it.
/// The lock is held until `WorkspaceReindexLockGuard::release()` closes the connection and ends the session.
/// Returning the connection to the pool instead would keep the session, and the lock, alive.
///
/// Does not time out, because a reindex of a large workspace can legitimately take long.
/// Log lines before and after acquisition make a wait visible.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn acquire_workspace_reindex_lock(
    pool: PgPool,
    workspace_id: Uuid,
) -> Result<WorkspaceReindexLockGuard, sea_orm::sqlx::Error> {
    // Reindexes serialize with each other, not with entity quota locks during provider calls.
    let key = workspace_reindex_lock_key(workspace_id);
    tracing::info!(%key, "waiting for workspace reindex lock");

    let conn = crate::db::postgres_session_lock(&pool, &key, true)
        .await?
        .ok_or_else(|| sea_orm::sqlx::Error::Protocol("blocking lock was not acquired".into()))?;

    tracing::info!(%key, "acquired workspace reindex lock");

    Ok(WorkspaceReindexLockGuard { conn: Some(conn) })
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
    let lock = if let Some(pool) = crate::db::reindex_lock_pool(ctx)? {
        Some(
            acquire_workspace_reindex_lock(pool, workspace_id)
                .await
                .map_err(|err| {
                    crate::YorishiroError::Internal(
                        format!("failed to acquire workspace lock: {err}").into(),
                    )
                })?,
        )
    } else {
        None
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
