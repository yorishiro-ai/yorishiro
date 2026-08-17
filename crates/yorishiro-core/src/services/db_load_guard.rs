//! Automatic read-only under database load (FR-9, layer 4).
//!
//! When the database stays busy past a threshold, the deployment drops to read-only rather than
//! waiting to become unresponsive: reads keep working, and writes get a `423` with a
//! `Retry-After` instead of a timeout. It goes back on its own once the load subsides.
//!
//! # The signal
//!
//! **CPU is not available from SQL.** `pg_stat_database` carries cumulative I/O time and
//! transaction counts, not an instantaneous load: measured, not assumed. The two candidates
//! were connection count and a webhook from external monitoring; this takes the first.
//!
//! It is an approximation and worth naming as one: the pool has a ceiling, so real saturation
//! shows up as queueing *behind* the pool rather than as more active connections. What this
//! catches is the shape that precedes it. A deployment that wants the true figure has monitoring
//! already and can drive `admin maintenance` from it; this exists so that a deployment with
//! nothing else keeps serving reads instead of falling over.

use std::time::Duration;

use sqlx::PgPool;
use tokio::time::interval;

use crate::error::{ResultExt, YorishiroError};
use crate::repositories::maintenance::{self, MaintenanceMode};

/// Written as the reason when this guard trips, and matched when deciding whether to lift.
///
/// The distinction matters: an operator who typed `admin maintenance read-only` before a restore
/// must not have it undone because the database happened to look quiet. Only a state this guard
/// wrote is one it may clear.
pub const AUTO_REASON: &str = "database load (automatic)";

pub struct LoadGuardConfig {
    /// Active connections above which the database counts as busy.
    pub threshold: i64,
    /// How long it must stay busy before dropping to read-only, and stay quiet before lifting.
    /// A single spike is not an outage, and flapping between modes is worse than either.
    pub sustain: Duration,
    pub poll: Duration,
}

impl LoadGuardConfig {
    /// `YORISHIRO_DB_LOAD_THRESHOLD` (default 0 = disabled), `YORISHIRO_DB_LOAD_SUSTAIN_SECS` (default 30),
    /// `YORISHIRO_DB_LOAD_POLL_SECS` (default 5).
    ///
    /// Off by default. Switching a deployment to read-only without being asked is a large thing
    /// to do on a default, and the right threshold depends on `max_connections`, which this
    /// crate does not choose.
    pub fn from_env() -> Option<Self> {
        let threshold: i64 = std::env::var("YORISHIRO_DB_LOAD_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if threshold <= 0 {
            return None;
        }
        Some(Self {
            threshold,
            sustain: Duration::from_secs(
                std::env::var("YORISHIRO_DB_LOAD_SUSTAIN_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(30),
            ),
            // `.filter(|v| *v > 0)` rather than `unwrap_or` alone: `tokio::time::interval`
            // panics on a zero period, and `0` is what an operator writes when they mean "off".
            // Falling back to the default keeps the guard running at 5s instead of taking the
            // process down: turning it off is what `YORISHIRO_DB_LOAD_THRESHOLD=0` is for.
            poll: Duration::from_secs(
                std::env::var("YORISHIRO_DB_LOAD_POLL_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .filter(|v| *v > 0)
                    .unwrap_or(5),
            ),
        })
    }
}

/// Active connections right now, this database only.
///
/// `state = 'active'` rather than every row: an idle connection holds a slot but is not load.
pub async fn active_connections(pool: &PgPool) -> Result<i64, YorishiroError> {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity \
         WHERE datname = current_database() AND state = 'active'",
    )
    .fetch_one(pool)
    .await
    .internal()
}

/// One decision. Separated from the loop so the rule is testable without waiting on a clock.
///
/// Returns the mode to switch to, or `None` to leave things alone.
pub fn decide(
    current: MaintenanceMode,
    current_reason: Option<&str>,
    busy_for: Duration,
    quiet_for: Duration,
    sustain: Duration,
) -> Option<MaintenanceMode> {
    match current {
        // Busy long enough, and nothing else is going on: step down.
        MaintenanceMode::Off if busy_for >= sustain => Some(MaintenanceMode::ReadOnly),
        // Quiet long enough, and this guard is what put it here: step back up.
        MaintenanceMode::ReadOnly
            if current_reason == Some(AUTO_REASON) && quiet_for >= sustain =>
        {
            Some(MaintenanceMode::Off)
        }
        // A full lock, or a read-only somebody else asked for. Not ours to touch.
        _ => None,
    }
}

/// Polls until the process ends. Spawned at startup when `LoadGuardConfig::from_env` yields one.
pub async fn run(pool: PgPool, config: LoadGuardConfig) {
    let mut ticker = interval(config.poll);
    let mut busy_for = Duration::ZERO;
    let mut quiet_for = Duration::ZERO;

    loop {
        ticker.tick().await;

        let active = match active_connections(&pool).await {
            Ok(n) => n,
            // A failure to measure is not a reason to change anything: the database being
            // unreachable is exactly when flipping modes would help least.
            Err(err) => {
                tracing::warn!(error = %err, "db load guard could not read pg_stat_activity");
                continue;
            }
        };

        if active >= config.threshold {
            busy_for += config.poll;
            quiet_for = Duration::ZERO;
        } else {
            quiet_for += config.poll;
            busy_for = Duration::ZERO;
        }

        let mut conn = match pool.acquire().await {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!(error = %err, "db load guard could not acquire a connection");
                continue;
            }
        };
        let state = match maintenance::get(&mut conn).await {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(error = %err, "db load guard could not read maintenance state");
                continue;
            }
        };
        drop(conn);

        let Some(next) = decide(
            state.mode,
            state.reason.as_deref(),
            busy_for,
            quiet_for,
            config.sustain,
        ) else {
            continue;
        };

        let reason = (next == MaintenanceMode::ReadOnly).then(|| AUTO_REASON.to_string());
        // Keeps whatever Retry-After the deployment already advertises: this guard decides
        // *when* to refuse, not how long to tell a caller to wait.
        match maintenance::set(&pool, next, state.retry_after, reason).await {
            Ok(_) => {
                tracing::warn!(
                    active_connections = active,
                    threshold = config.threshold,
                    mode = ?next,
                    "db load guard switched maintenance mode"
                );
                busy_for = Duration::ZERO;
                quiet_for = Duration::ZERO;
            }
            Err(err) => tracing::error!(error = %err, "db load guard could not switch mode"),
        }
    }
}

#[cfg(test)]
#[path = "../../tests/services/db_load_guard.rs"]
mod tests;
