//! Automatic read-only under database load.
//!
//! When the database stays busy past a threshold, the deployment drops to read-only rather than
//! waiting to become unresponsive: reads keep working, and writes get a 423 with a Retry-After
//! instead of a timeout. It goes back on its own once the load subsides.
//!
//! This was never ported across the Loco rebuild — `src/services/maintenance.rs` only implements
//! the manual toggle's read/restore. The old `sqlx`-based `db_load_guard` lived in
//! `crates/yorishiro-core/src/services/db_load_guard.rs` and was dropped in the rebuild commit.
//!
//! The implementation is based on the old design: poll `pg_stat_activity` for active connections,
//! trip when sustained past a threshold, and revert when quiet.

use loco_rs::app::AppContext;

use crate::db::DbHandle;
use crate::error::YorishiroError;
use crate::models::system_maintenance::{self, MaintenanceMode, MaintenanceState};

/// The threshold at which the guard considers the database busy.
///
/// 80% of `max_connections` from the config, so the pool still has headroom for health checks
/// and the guard's own queries before true saturation.
const DEFAULT_THRESHOLD_PERCENT: f64 = 0.8;

/// Reason string written when this guard enables maintenance mode.
///
/// Used to distinguish guard-driven mode from operator-driven mode: the guard only disables
/// a mode it itself enabled.
pub const AUTO_REASON: &str = "database load (automatic)";

/// Reads the current number of active connections from the identity pool.
///
/// `state = 'active'` rather than every row: an idle connection holds a slot but is not load.
/// Runs against the identity pool (`DbHandle::identity`), which the migration role connects with
/// and which always has access to `pg_stat_activity`.
async fn active_connections(handle: &DbHandle) -> Result<i64, YorishiroError> {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity \
         WHERE datname = current_database() AND state = 'active'",
    )
    .fetch_one(&handle.identity)
    .await
    .map_err(|e| YorishiroError::Internal(anyhow::anyhow!(e.to_string())))
}

/// Whether the load has subsided enough to lift read-only (if we set it).
fn should_lift(current: &MaintenanceState) -> bool {
    current.mode == MaintenanceMode::ReadOnly
        && current.reason.as_deref() == Some(AUTO_REASON)
}

/// Reads pool saturation and, if it crosses the threshold, enables read-only maintenance mode.
///
/// This is the periodic check that a worker/task calls. It reads the current maintenance state,
/// measures active connections against the pool's configured max, and calls
/// `system_maintenance::set` when the threshold is crossed.
///
/// Runs on the identity pool (the migration role), never the RLS-scoped tenant pool.
///
/// # Errors
/// Returns an error if it cannot read the maintenance state or connection counts.
/// These are logged and suppressed in the worker loop, not propagated.
pub async fn check_and_maybe_enable_readonly(ctx: &AppContext) -> loco_rs::Result<()> {
    // On SQLite there is no second pool and no RLS — nothing to protect.
    if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return Ok(());
    }

    let handle = ctx
        .shared_store
        .get::<DbHandle>()
        .ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!(
                "db_load_guard: DbHandle not found in shared_store"
            ))
        })?;

    // Read max_connections from config.
    let max_conns = ctx.config.database.max_connections as f64;
    if max_conns <= 0.0 {
        return Ok(());
    }

    let threshold = (max_conns * DEFAULT_THRESHOLD_PERCENT).ceil() as i64;

    let active = active_connections(&handle).await?;

    if active < threshold {
        // Load is under threshold. Check if we should lift read-only that we set ourselves.
        let current = system_maintenance::get(&ctx.db).await
            .map_err(|e| YorishiroError::Internal(anyhow::anyhow!(e.to_string())))?;
        if should_lift(&current) {
            system_maintenance::set(&ctx.db, MaintenanceMode::Off, 300, None)
                .await
                .map_err(|e| YorishiroError::Internal(anyhow::anyhow!(e.to_string())))?;
            tracing::info!(
                active_connections = active,
                threshold = threshold,
                "db_load_guard: lifted read-only (load subsided)"
            );
        }
        return Ok(());
    }

    // Load is at or above threshold. Enable read-only if we are not already.
    let current = system_maintenance::get(&ctx.db).await
        .map_err(|e| YorishiroError::Internal(anyhow::anyhow!(e.to_string())))?;

    if current.mode != MaintenanceMode::Off {
        return Ok(());
    }

    system_maintenance::set(
        &ctx.db,
        MaintenanceMode::ReadOnly,
        300,
        Some(AUTO_REASON.to_string()),
    )
    .await
    .map_err(|e| YorishiroError::Internal(anyhow::anyhow!(e.to_string())))?;

    tracing::warn!(
        active_connections = active,
        threshold = threshold,
        "db_load_guard: enabled read-only (load sustained)"
    );

    Ok(())
}

#[cfg(test)]
#[path = "../../tests/services/db_load_guard.rs"]
mod tests;
