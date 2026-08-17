use std::time::Duration;

use sqlx::PgPool;

use crate::repositories::maintenance::MaintenanceMode;
use crate::services::db_load_guard::{AUTO_REASON, LoadGuardConfig, active_connections, decide};

const SUSTAIN: Duration = Duration::from_secs(30);
const LONG: Duration = Duration::from_secs(31);
const SHORT: Duration = Duration::from_secs(5);

#[test]
fn drops_to_read_only_only_after_the_load_is_sustained() {
    assert_eq!(
        decide(MaintenanceMode::Off, None, SHORT, Duration::ZERO, SUSTAIN),
        None,
        "a spike is not an outage"
    );
    assert_eq!(
        decide(MaintenanceMode::Off, None, LONG, Duration::ZERO, SUSTAIN),
        Some(MaintenanceMode::ReadOnly)
    );
}

/// The distinction the whole design turns on: an operator who asked for read-only before a restore must not have it lifted because the database went quiet.
#[test]
fn lifts_only_what_it_set_itself() {
    assert_eq!(
        decide(
            MaintenanceMode::ReadOnly,
            Some(AUTO_REASON),
            Duration::ZERO,
            LONG,
            SUSTAIN
        ),
        Some(MaintenanceMode::Off),
        "its own state comes back on its own"
    );
    assert_eq!(
        decide(
            MaintenanceMode::ReadOnly,
            Some("restoring a backup"),
            Duration::ZERO,
            LONG,
            SUSTAIN
        ),
        None,
        "somebody else's read-only stays"
    );
    assert_eq!(
        decide(
            MaintenanceMode::ReadOnly,
            None,
            Duration::ZERO,
            LONG,
            SUSTAIN
        ),
        None,
        "a read-only with no reason is not ours either"
    );
}

/// A full lock is a deliberate act: a restore, a migration.
/// Load has nothing to say about it.
#[test]
fn never_touches_a_full_lock() {
    assert_eq!(
        decide(
            MaintenanceMode::FullLock,
            Some(AUTO_REASON),
            LONG,
            LONG,
            SUSTAIN
        ),
        None
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn counts_the_connections_it_claims_to(pool: PgPool) {
    // This test's own connection is active while the query runs, so the floor is 1.
    let n = active_connections(&pool).await.unwrap();
    assert!(n >= 1, "at least this query is active, got {n}");
}

/// Serializes the tests below, which set process-wide environment variables.
/// Same shape and same reasoning as `MaxTenantsGuard` in `tests/repositories/tenancy/tenants.rs`, including taking the lock with `unwrap_or_else(|e| e.into_inner())` so one panicking test does not cascade into unrelated failures.
static LOAD_GUARD_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct LoadGuardEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
    /// What each variable held before this guard ran, so `Drop` puts it back.
    /// Removing them unconditionally would delete a value the test process was started with, and the next test to read either one would see a different deployment than the one it was run under.
    previous: [(&'static str, Option<std::ffi::OsString>); 2],
}

impl LoadGuardEnv {
    /// The threshold is always set, since `from_env` returns `None` without it and there would be no config to inspect.
    fn set(poll: Option<&str>) -> Self {
        let lock = LOAD_GUARD_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let previous = [
            (
                "YORISHIRO_DB_LOAD_THRESHOLD",
                std::env::var_os("YORISHIRO_DB_LOAD_THRESHOLD"),
            ),
            (
                "YORISHIRO_DB_LOAD_POLL_SECS",
                std::env::var_os("YORISHIRO_DB_LOAD_POLL_SECS"),
            ),
        ];
        // SAFETY: serialized by LOAD_GUARD_ENV_LOCK, and nothing else touches these keys.
        unsafe {
            std::env::set_var("YORISHIRO_DB_LOAD_THRESHOLD", "10");
            match poll {
                Some(v) => std::env::set_var("YORISHIRO_DB_LOAD_POLL_SECS", v),
                None => std::env::remove_var("YORISHIRO_DB_LOAD_POLL_SECS"),
            }
        }
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for LoadGuardEnv {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            // SAFETY: serialized by LOAD_GUARD_ENV_LOCK, and nothing else touches these keys.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

/// `tokio::time::interval` panics on a zero period, and `0` is what an operator writes when they mean "off": so the value that reaches it must never be zero, whatever the variable says.
/// Turning the guard off is `YORISHIRO_DB_LOAD_THRESHOLD=0`, which `from_env` already honours by returning `None`.
#[test]
fn a_zero_poll_interval_does_not_reach_the_ticker() {
    let _guard = LoadGuardEnv::set(Some("0"));
    let config = LoadGuardConfig::from_env().expect("a positive threshold configures the guard");
    assert_eq!(
        config.poll,
        Duration::from_secs(5),
        "zero must fall back to the default, not through to interval()"
    );
}

/// The fallback is not a blanket one: a poll interval an operator did set has to survive.
#[test]
fn a_positive_poll_interval_is_kept() {
    let _guard = LoadGuardEnv::set(Some("11"));
    let config = LoadGuardConfig::from_env().expect("a positive threshold configures the guard");
    assert_eq!(config.poll, Duration::from_secs(11));
}
