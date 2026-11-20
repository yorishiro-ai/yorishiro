#![expect(
    clippy::disallowed_methods,
    reason = "tests read their inputs, such as DATABASE_URL and the licence key, from the environment"
)]

mod controllers;
mod data;
mod db;
#[cfg(feature = "enterprise")]
mod ee;
mod error;
mod http_stub;
mod initializers;
mod integration;

mod models;
mod requests;
mod services;
mod tasks;
mod workers;

async fn has_active_job(
    db: &impl sea_orm::ConnectionTrait,
    job_name: &str,
    workspace_id: uuid::Uuid,
) -> Result<bool, sea_orm::DbErr> {
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
    use yorishiro::models::queue_job_lifecycles::{Column, Entity, LifecycleStatus};

    Ok(Entity::find()
        .filter(Column::JobName.eq(job_name))
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Status.is_in([
            LifecycleStatus::Queued.as_db_str(),
            LifecycleStatus::Running.as_db_str(),
            LifecycleStatus::Retrying.as_db_str(),
        ]))
        .count(db)
        .await?
        > 0)
}

/// A SQLite application config for fixtures, whichever backend the suite runs on.
/// The ambient `DATABASE_URL` and queue settings are replaced while the test configuration renders, then restored.
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until configuration rendering completes"
)]
pub(crate) fn sqlite_fixture_config(uri: &str) -> loco_rs::config::Config {
    let guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_QUEUE_KIND"]);
    guard.set("DATABASE_URL", uri);
    guard.set("QUEUE_URL", "sqlite::memory:");
    guard.remove("YORISHIRO_QUEUE_KIND");
    let config = loco_rs::environment::Environment::Test
        .load()
        .expect("render SQLite fixture configuration");
    yorishiro::data::config::validate_queue_policy(&config).expect("validate SQLite fixture pair");
    config
}

use std::env as process_env;
use std::ffi::{OsStr, OsString};

use serial_test::serial;

/// Orders every test that boots the application (readers) against every test that rewrites the process environment (writers).
///
/// `serial_test` keys only exclude tests that carry the same key, so a test with no key still reads `DATABASE_URL`, `QUEUE_URL` and the rest while it boots.
/// A writer therefore waits for the booting tests in flight and holds the others off until its [`EnvGuard`] is dropped.
///
/// A nested read shares the environment lock already held on the same thread.
/// Nested environment guards share that write lock until the outer guard exits.
/// A write inside a read is a bug in the test: it would wait on itself, so it is refused loudly.
/// Capture the [`EnvGuard`] before the call that boots the application.
static ENVIRONMENT: std::sync::RwLock<()> = std::sync::RwLock::new(());

thread_local! {
    static SCOPES: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
}

/// Held for as long as a test reads the process environment while it runs.
pub(crate) struct EnvironmentRead {
    lock: Option<std::sync::RwLockReadGuard<'static, ()>>,
}

impl EnvironmentRead {
    pub(crate) fn enter() -> Self {
        let held = SCOPES.with(|scopes| scopes.get() != (0, 0));
        let lock = (!held).then(|| {
            ENVIRONMENT
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        });
        SCOPES.with(|scopes| {
            let (readers, writers) = scopes.get();
            scopes.set((readers + 1, writers));
        });
        Self { lock }
    }
}

impl Drop for EnvironmentRead {
    fn drop(&mut self) {
        SCOPES.with(|scopes| {
            let (readers, writers) = scopes.get();
            scopes.set((readers - 1, writers));
        });
    }
}

struct EnvironmentWrite {
    _lock: Option<std::sync::RwLockWriteGuard<'static, ()>>,
}

#[test]
fn an_environment_reader_can_nest_inside_a_writer() {
    let _write = EnvironmentWrite::enter();
    let read = EnvironmentRead::enter();
    assert!(read.lock.is_none(), "the writer already owns the lock");
}

#[test]
fn an_environment_write_inside_a_reader_is_refused() {
    let _read = EnvironmentRead::enter();
    let result = std::panic::catch_unwind(EnvironmentWrite::enter);
    assert!(result.is_err(), "a write cannot bypass a held read lock");
    drop(result);
    assert_eq!(SCOPES.with(std::cell::Cell::get), (1, 0));
}

impl EnvironmentWrite {
    fn enter() -> Self {
        let (readers, writers) = SCOPES.with(std::cell::Cell::get);
        assert!(
            readers == 0 || writers > 0,
            "an EnvGuard was captured inside a booted test's scope: capture it before boot_request"
        );
        let lock = (readers == 0 && writers == 0).then(|| {
            ENVIRONMENT
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        });
        SCOPES.with(|scopes| scopes.set((readers, writers + 1)));
        Self { _lock: lock }
    }
}

impl Drop for EnvironmentWrite {
    fn drop(&mut self) {
        SCOPES.with(|scopes| {
            let (readers, writers) = scopes.get();
            scopes.set((readers, writers - 1));
        });
    }
}

/// Captures process environment values and restores them when the test exits.
///
/// The guard is intentionally Drop-based so restoration also runs while a test unwinds from a
/// panic.
/// Named `serial_test` locks and process environments are local to each test executable.
pub(crate) struct EnvGuard {
    values: Vec<(&'static str, Option<OsString>)>,
    // Dropped after `Drop::drop` has restored the values, so the environment is whole again before readers resume.
    _scope: EnvironmentWrite,
}

impl EnvGuard {
    pub(crate) fn capture(variables: &[&'static str]) -> Self {
        let scope = EnvironmentWrite::enter();
        Self {
            _scope: scope,
            values: variables
                .iter()
                .map(|variable| (*variable, process_env::var_os(variable)))
                .collect(),
        }
    }

    pub(crate) fn set(&self, variable: &'static str, value: impl AsRef<OsStr>) {
        assert!(
            self.values.iter().any(|(name, _)| *name == variable),
            "environment variable {variable} was not captured"
        );
        // SAFETY: callers hold the `process_environment` serial_test lock while this guard is live.
        unsafe { process_env::set_var(variable, value) };
    }

    pub(crate) fn remove(&self, variable: &'static str) {
        assert!(
            self.values.iter().any(|(name, _)| *name == variable),
            "environment variable {variable} was not captured"
        );
        // SAFETY: callers hold the `process_environment` serial_test lock while this guard is live.
        unsafe { process_env::remove_var(variable) };
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: the guard's test owns the `process_environment` serial_test lock until Drop.
        unsafe {
            for (variable, original) in &self.values {
                match original {
                    Some(value) => process_env::set_var(variable, value),
                    None => process_env::remove_var(variable),
                }
            }
        }
    }
}

#[test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
fn env_guard_restores_original_value_on_normal_drop() {
    const VARIABLE: &str = "YORISHIRO_TEST_ENV_GUARD";

    let outer = EnvGuard::capture(&[VARIABLE]);
    let original = OsString::from("original-value");
    outer.set(VARIABLE, &original);

    {
        let guard = EnvGuard::capture(&[VARIABLE]);
        guard.set(VARIABLE, "temporary-value");
    }

    assert_eq!(process_env::var_os(VARIABLE), Some(original));
}

#[test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
fn env_guard_restores_original_value_during_unwind() {
    const VARIABLE: &str = "YORISHIRO_TEST_ENV_GUARD";

    let outer = EnvGuard::capture(&[VARIABLE]);
    let original = OsString::from("original-value");
    outer.set(VARIABLE, &original);

    let result = std::panic::catch_unwind(|| {
        let guard = EnvGuard::capture(&[VARIABLE]);
        guard.set(VARIABLE, "temporary-value");
        panic!("exercise EnvGuard unwind restoration");
    });

    assert!(result.is_err());
    assert_eq!(process_env::var_os(VARIABLE), Some(original));
}

fn is_backend(expected: &str) -> bool {
    let _environment = EnvironmentRead::enter();
    let url = std::env::var("DATABASE_URL").unwrap_or_default();
    let prefixes: &[&str] = if expected == "sqlite" {
        &["sqlite:"]
    } else {
        &["postgres://", "postgresql://"]
    };
    prefixes.iter().any(|prefix| url.starts_with(prefix))
}

/// Backend-specific tests return early when another backend is active.
pub(crate) fn require_sqlite_backend() -> bool {
    is_backend("sqlite")
}

pub(crate) fn require_postgres_backend() -> bool {
    is_backend("postgres")
}
