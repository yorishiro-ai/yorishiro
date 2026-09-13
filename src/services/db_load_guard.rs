//! Automatic read-only mode under sustained PostgreSQL connection load.
//!
//! This is an opt-in safeguard because changing a deployment-wide maintenance state without an operator request is a significant operational action.
//! It ports the pre-Loco guard's threshold, sustain window, ownership marker, and safe polling fallback onto the Loco two-pool architecture.

use std::time::{Duration, Instant};

use loco_rs::app::AppContext;
use sqlx::PgPool;
use tokio::time::interval;

use crate::db::DbHandle;
use crate::error::YorishiroError;
use crate::models::system_maintenance::{self, MaintenanceMode, MaintenanceState};

/// Reason string written when this guard enables maintenance mode.
pub const AUTO_REASON: &str = "database load (automatic)";

/// Runtime settings for the load guard.
pub struct LoadGuardConfig {
    /// Active connections at or above which the database counts as busy.
    pub threshold: i64,
    /// How long the database must remain busy or quiet before changing mode.
    pub sustain: Duration,
    /// Time between activity samples.
    pub poll: Duration,
}

impl LoadGuardConfig {
    /// Reads the opt-in guard settings.
    ///
    /// `YORISHIRO_DB_LOAD_THRESHOLD` defaults to zero, which disables the guard.
    /// `YORISHIRO_DB_LOAD_SUSTAIN_SECS` defaults to 30 and `YORISHIRO_DB_LOAD_POLL_SECS` defaults to 5.
    pub fn from_env() -> Option<Self> {
        let threshold = std::env::var("YORISHIRO_DB_LOAD_THRESHOLD")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        if threshold <= 0 {
            return None;
        }

        Some(Self {
            threshold,
            sustain: Duration::from_secs(
                std::env::var("YORISHIRO_DB_LOAD_SUSTAIN_SECS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(30),
            ),
            poll: Duration::from_secs(
                std::env::var("YORISHIRO_DB_LOAD_POLL_SECS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(5),
            ),
        })
    }
}

/// Returns the maintenance transition allowed by the current load history.
pub fn decide(
    current: &MaintenanceState,
    busy_for: Duration,
    quiet_for: Duration,
    sustain: Duration,
) -> Option<MaintenanceMode> {
    match current.mode {
        MaintenanceMode::Off if busy_for >= sustain => Some(MaintenanceMode::ReadOnly),
        MaintenanceMode::ReadOnly
            if current.reason.as_deref() == Some(AUTO_REASON) && quiet_for >= sustain =>
        {
            Some(MaintenanceMode::Off)
        }
        _ => None,
    }
}

#[derive(Default)]
struct LoadHistory {
    busy_since: Option<Instant>,
    quiet_since: Option<Instant>,
}

impl LoadHistory {
    fn reset(&mut self) {
        self.busy_since = None;
        self.quiet_since = None;
    }

    fn observe(&mut self, busy: bool, now: Instant) -> (Duration, Duration) {
        if busy {
            if self.busy_since.is_none() {
                self.busy_since = Some(now);
            }
            self.quiet_since = None;
        } else {
            if self.quiet_since.is_none() {
                self.quiet_since = Some(now);
            }
            self.busy_since = None;
        }

        (
            self.busy_since.map_or(Duration::ZERO, |since| now - since),
            self.quiet_since.map_or(Duration::ZERO, |since| now - since),
        )
    }
}

async fn active_connections(pool: &PgPool) -> Result<i64, YorishiroError> {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity \
         WHERE datname = current_database() AND state = 'active'",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| YorishiroError::Internal(anyhow::anyhow!(error)))
}

async fn poll(ctx: &AppContext) -> Result<i64, YorishiroError> {
    if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return Ok(0);
    }

    let handle = ctx.shared_store.get::<DbHandle>().ok_or_else(|| {
        YorishiroError::Internal(anyhow::anyhow!(
            "db_load_guard: DbHandle not found in shared_store"
        ))
    })?;
    let active = active_connections(&handle.identity).await?;
    Ok(active)
}

/// Runs the opt-in guard until the process exits.
pub async fn run(ctx: AppContext, config: LoadGuardConfig) {
    let mut ticker = interval(config.poll);
    let mut history = LoadHistory::default();

    loop {
        ticker.tick().await;
        if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
            return;
        }

        let Some(handle) = ctx.shared_store.get::<DbHandle>() else {
            tracing::warn!("db_load_guard: DbHandle not found in shared_store");
            return;
        };
        let active = match active_connections(&handle.identity).await {
            Ok(active) => active,
            Err(error) => {
                tracing::warn!(error = %error, "db_load_guard: could not read pg_stat_activity");
                history.reset();
                continue;
            }
        };

        let (busy_for, quiet_for) = history.observe(active >= config.threshold, Instant::now());

        let current = match system_maintenance::get(&ctx.db).await {
            Ok(current) => current,
            Err(error) => {
                tracing::warn!(error = %error, "db_load_guard: could not read maintenance state");
                history.reset();
                continue;
            }
        };
        let Some(mode) = decide(&current, busy_for, quiet_for, config.sustain) else {
            continue;
        };

        let reason = (mode == MaintenanceMode::ReadOnly).then(|| AUTO_REASON.to_string());
        match system_maintenance::set_if_current(
            &ctx.db,
            &current,
            mode,
            current.retry_after,
            reason,
        )
        .await
        {
            Ok(true) => {
                tracing::warn!(active_connections = active, threshold = config.threshold, mode = ?mode, "db_load_guard: switched maintenance mode");
                history.reset();
            }
            Ok(false) => tracing::info!(
                "db_load_guard: maintenance state changed concurrently; transition skipped"
            ),
            Err(error) => {
                tracing::error!(error = %error, "db_load_guard: could not switch maintenance mode")
            }
        }
    }
}

/// Performs one diagnostic task invocation without changing maintenance mode.
pub async fn check_once(ctx: &AppContext) -> loco_rs::Result<()> {
    let Some(config) = LoadGuardConfig::from_env() else {
        return Ok(());
    };
    let active = poll(ctx).await?;
    tracing::info!(
        active_connections = active,
        threshold = config.threshold,
        "db_load_guard: check completed"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(mode: MaintenanceMode, reason: Option<&str>) -> MaintenanceState {
        MaintenanceState {
            mode,
            retry_after: 300,
            reason: reason.map(str::to_owned),
        }
    }

    #[test]
    fn drops_to_read_only_only_after_sustained_load() {
        let sustain = Duration::from_secs(30);
        assert_eq!(
            decide(
                &state(MaintenanceMode::Off, None),
                Duration::from_secs(5),
                Duration::ZERO,
                sustain
            ),
            None
        );
        assert_eq!(
            decide(
                &state(MaintenanceMode::Off, None),
                sustain,
                Duration::ZERO,
                sustain
            ),
            Some(MaintenanceMode::ReadOnly)
        );
    }

    #[test]
    fn lifts_only_guard_owned_read_only() {
        let sustain = Duration::from_secs(30);
        assert_eq!(
            decide(
                &state(MaintenanceMode::ReadOnly, Some(AUTO_REASON)),
                Duration::ZERO,
                sustain,
                sustain
            ),
            Some(MaintenanceMode::Off)
        );
        assert_eq!(
            decide(
                &state(MaintenanceMode::ReadOnly, Some("restore")),
                Duration::ZERO,
                sustain,
                sustain
            ),
            None
        );
        assert_eq!(
            decide(
                &state(MaintenanceMode::FullLock, Some(AUTO_REASON)),
                sustain,
                sustain,
                sustain
            ),
            None
        );
    }

    #[test]
    fn first_sample_does_not_count_as_poll_time() {
        let sustain = Duration::from_secs(30);
        let start = Instant::now();
        let mut history = LoadHistory::default();
        let (busy_for, quiet_for) = history.observe(true, start);
        assert_eq!((busy_for, quiet_for), (Duration::ZERO, Duration::ZERO));
        let (busy_for, quiet_for) = history.observe(true, start + Duration::from_secs(29));
        assert_eq!(quiet_for, Duration::ZERO);
        assert_eq!(
            decide(
                &state(MaintenanceMode::Off, None),
                busy_for,
                quiet_for,
                sustain
            ),
            None
        );
        let (busy_for, quiet_for) = history.observe(true, start + sustain);
        assert_eq!(quiet_for, Duration::ZERO);
        assert_eq!(
            decide(
                &state(MaintenanceMode::Off, None),
                busy_for,
                quiet_for,
                sustain
            ),
            Some(MaintenanceMode::ReadOnly)
        );
    }

    #[test]
    fn measurement_error_resets_the_sustain_window() {
        let sustain = Duration::from_secs(30);
        let start = Instant::now();
        let mut history = LoadHistory::default();
        history.observe(true, start);
        history.reset();

        let (busy_for, quiet_for) = history.observe(true, start + sustain);
        assert_eq!((busy_for, quiet_for), (Duration::ZERO, Duration::ZERO));
        assert_eq!(
            decide(
                &state(MaintenanceMode::Off, None),
                busy_for,
                quiet_for,
                sustain
            ),
            None
        );

        let (busy_for, quiet_for) = history.observe(true, start + sustain + sustain);
        assert_eq!(quiet_for, Duration::ZERO);
        assert_eq!(
            decide(
                &state(MaintenanceMode::Off, None),
                busy_for,
                quiet_for,
                sustain
            ),
            Some(MaintenanceMode::ReadOnly)
        );
    }

    #[test]
    #[serial_test::serial]
    fn zero_poll_interval_falls_back_to_safe_default() {
        let previous_threshold = std::env::var_os("YORISHIRO_DB_LOAD_THRESHOLD");
        let previous = std::env::var_os("YORISHIRO_DB_LOAD_POLL_SECS");
        unsafe {
            std::env::set_var("YORISHIRO_DB_LOAD_THRESHOLD", "10");
            std::env::set_var("YORISHIRO_DB_LOAD_POLL_SECS", "0");
        }
        assert_eq!(
            LoadGuardConfig::from_env().unwrap().poll,
            Duration::from_secs(5)
        );
        unsafe {
            match previous {
                Some(value) => std::env::set_var("YORISHIRO_DB_LOAD_POLL_SECS", value),
                None => std::env::remove_var("YORISHIRO_DB_LOAD_POLL_SECS"),
            }
            match previous_threshold {
                Some(value) => std::env::set_var("YORISHIRO_DB_LOAD_THRESHOLD", value),
                None => std::env::remove_var("YORISHIRO_DB_LOAD_THRESHOLD"),
            }
        }
    }
}
