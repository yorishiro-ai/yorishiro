//! Automatic read-only mode under sustained PostgreSQL connection load.
//!
//! This is an opt-in safeguard because changing a deployment-wide maintenance state without an operator request is a significant operational action.
//! It ports the pre-Loco guard's threshold, sustain window, ownership marker, and safe polling fallback onto the Loco two-pool architecture.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::Router as AxumRouter;
use loco_rs::{
    Result,
    app::{AppContext, Initializer},
    environment::Environment,
};
use tokio::task::JoinHandle;
use tokio::time::interval;

use crate::db::{self, DbHandle};
use crate::error::YorishiroError;
use crate::models::system_maintenance::{self, AUTO_REASON, MaintenanceMode, MaintenanceState};

/// Starts the load monitor in every process that serves HTTP.
///
/// `after_routes` runs only for start modes that build a router, so queue workers never run it; test boots skip it because the monitor would outlive the test's own database.
pub(crate) struct LoadGuard;

#[async_trait]
impl Initializer for LoadGuard {
    fn name(&self) -> String {
        "db_load_guard".into()
    }

    async fn after_routes(&self, router: AxumRouter, ctx: &AppContext) -> Result<AxumRouter> {
        if matches!(ctx.environment, Environment::Test) {
            return Ok(router);
        }
        let settings = ctx
            .shared_store
            .get::<crate::data::settings::Settings>()
            .ok_or_else(|| {
                loco_rs::Error::Message("application settings were not installed".into())
            })?;
        if let Some(config) = LoadGuardConfig::from_settings(&settings.db_load_guard) {
            let task_ctx = ctx.clone();
            let task = tokio::spawn(async move { run(task_ctx, config).await });
            ctx.shared_store.insert(Monitor(task));
        }
        Ok(router)
    }
}

/// The running monitor, kept in `shared_store` so shutdown can stop it and wait for it.
struct Monitor(JoinHandle<()>);

/// Stops the monitor, if one was started, and waits for it to finish.
pub(crate) async fn shutdown(ctx: &AppContext) {
    if let Some(Monitor(task)) = ctx.shared_store.remove::<Monitor>() {
        task.abort();
        let _ = task.await;
    }
}

/// Runtime settings for the load guard.
pub(crate) struct LoadGuardConfig {
    /// Active connections at or above which the database counts as busy.
    threshold: i64,
    /// How long the database must remain busy or quiet before changing mode.
    sustain: Duration,
    /// Time between activity samples.
    poll: Duration,
}

impl LoadGuardConfig {
    /// Reads the opt-in guard settings.
    ///
    /// `YORISHIRO_DB_LOAD_THRESHOLD` defaults to zero, which disables the guard.
    /// `YORISHIRO_DB_LOAD_SUSTAIN_SECS` defaults to 30 and `YORISHIRO_DB_LOAD_POLL_SECS` defaults to 5.
    pub(crate) fn from_settings(settings: &crate::data::settings::DbLoadGuard) -> Option<Self> {
        if settings.threshold <= 0 {
            return None;
        }

        Some(Self {
            threshold: settings.threshold,
            sustain: Duration::from_secs(settings.sustain_seconds),
            poll: Duration::from_secs(settings.poll_seconds),
        })
    }
}

/// Returns the maintenance transition allowed by the current load history.
fn decide(
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

enum StepOutcome {
    Continue,
    Transition {
        current: MaintenanceState,
        mode: MaintenanceMode,
    },
}

fn run_step(
    history: &mut LoadHistory,
    active: Result<i64, ()>,
    current: Result<MaintenanceState, ()>,
    threshold: i64,
    sustain: Duration,
    now: Instant,
) -> StepOutcome {
    let Ok(active) = active else {
        history.reset();
        return StepOutcome::Continue;
    };

    let (busy_for, quiet_for) = history.observe(active >= threshold, now);
    let Ok(current) = current else {
        history.reset();
        return StepOutcome::Continue;
    };

    let Some(mode) = decide(&current, busy_for, quiet_for, sustain) else {
        return StepOutcome::Continue;
    };
    StepOutcome::Transition { current, mode }
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
    let active = db::active_connections(&handle.identity).await?;
    Ok(active)
}

/// Runs the opt-in guard until the process exits.
pub(crate) async fn run(ctx: AppContext, config: LoadGuardConfig) {
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
        let active = match db::active_connections(&handle.identity).await {
            Ok(active) => Ok(active),
            Err(error) => {
                tracing::warn!(error = %error, "db_load_guard: could not read pg_stat_activity");
                Err(())
            }
        };
        let current = match active {
            Ok(_) => match system_maintenance::get(&ctx.db).await {
                Ok(current) => Ok(current),
                Err(error) => {
                    tracing::warn!(error = %error, "db_load_guard: could not read maintenance state");
                    Err(())
                }
            },
            Err(()) => Err(()),
        };
        match run_step(
            &mut history,
            active,
            current,
            config.threshold,
            config.sustain,
            Instant::now(),
        ) {
            StepOutcome::Continue => {}
            StepOutcome::Transition { current, mode } => {
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
                        tracing::warn!(active_connections = active.unwrap_or_default(), threshold = config.threshold, mode = ?mode, "db_load_guard: switched maintenance mode");
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
    }
}

/// Performs one diagnostic task invocation without changing maintenance mode.
pub(crate) async fn check_once(ctx: &AppContext) -> loco_rs::Result<()> {
    let settings = ctx.config.settings::<crate::data::settings::Settings>()?;
    let Some(config) = LoadGuardConfig::from_settings(&settings.db_load_guard) else {
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
    fn measurement_error_resets_the_sustain_window_through_run_step() {
        let sustain = Duration::from_secs(30);
        let start = Instant::now();
        let mut history = LoadHistory::default();
        let current = state(MaintenanceMode::Off, None);
        assert!(matches!(
            run_step(
                &mut history,
                Ok(10),
                Ok(current.clone()),
                10,
                sustain,
                start
            ),
            StepOutcome::Continue
        ));
        assert!(matches!(
            run_step(&mut history, Err(()), Err(()), 10, sustain, start + sustain),
            StepOutcome::Continue
        ));
        assert!(matches!(
            run_step(
                &mut history,
                Ok(10),
                Ok(current.clone()),
                10,
                sustain,
                start + sustain + sustain - Duration::from_secs(1)
            ),
            StepOutcome::Continue
        ));
        assert!(matches!(
            run_step(
                &mut history,
                Ok(10),
                Ok(current),
                10,
                sustain,
                start + sustain + sustain + sustain
            ),
            StepOutcome::Transition {
                mode: MaintenanceMode::ReadOnly,
                ..
            }
        ));
    }

    #[test]
    fn maintenance_state_error_resets_the_sustain_window_through_run_step() {
        let sustain = Duration::from_secs(30);
        let start = Instant::now();
        let mut history = LoadHistory::default();
        let current = state(MaintenanceMode::Off, None);
        assert!(matches!(
            run_step(
                &mut history,
                Ok(10),
                Ok(current.clone()),
                10,
                sustain,
                start
            ),
            StepOutcome::Continue
        ));
        assert!(matches!(
            run_step(&mut history, Ok(10), Err(()), 10, sustain, start + sustain),
            StepOutcome::Continue
        ));
        assert!(matches!(
            run_step(
                &mut history,
                Ok(10),
                Ok(current.clone()),
                10,
                sustain,
                start + sustain + sustain - Duration::from_secs(1)
            ),
            StepOutcome::Continue
        ));
        assert!(matches!(
            run_step(
                &mut history,
                Ok(10),
                Ok(current),
                10,
                sustain,
                start + sustain + sustain + sustain
            ),
            StepOutcome::Transition {
                mode: MaintenanceMode::ReadOnly,
                ..
            }
        ));
    }
}
