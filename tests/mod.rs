mod controllers;
mod data;
mod db;
mod db_enum;
#[cfg(feature = "enterprise")]
mod ee;
mod initializers;
mod integration;

mod config;
mod licence;
mod metaschema;
mod migration;
mod models;
mod requests;
mod services;
mod tasks;
mod workers;

/// Records the database and queue provider after Loco has booted the app.
pub(crate) fn record_configured_topology(ctx: &loco_rs::app::AppContext) {
    let config = &ctx.config;
    assert!(
        ctx.queue_provider.is_some(),
        "booted app has no queue provider"
    );
    let database = if config.database.uri.starts_with("sqlite:") {
        "sqlite"
    } else if config.database.uri.starts_with("postgres://")
        || config.database.uri.starts_with("postgresql://")
    {
        "postgres"
    } else {
        "unknown"
    };
    let queue = match config.queue.as_ref() {
        Some(loco_rs::config::QueueConfig::Postgres(_)) => "postgres",
        Some(loco_rs::config::QueueConfig::Sqlite(_)) => "sqlite",
        Some(loco_rs::config::QueueConfig::Redis(_)) => "valkey",
        _ => "unknown",
    };
    let Some(root) = std::env::var_os("YORISHIRO_TOPOLOGY_MARKER_DIR") else {
        return;
    };
    let path = std::path::Path::new(&root)
        .join(database)
        .join(queue)
        .join("configured-app");
    std::fs::create_dir_all(&path).expect("create topology marker directory");
    let file = path.join(format!(
        "{}-{}",
        std::process::id(),
        BACKEND_MARKER_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(file, b"executed\n").expect("write topology marker");
}

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use serial_test::serial;

static BACKEND_MARKER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Orders every test that boots the application (readers) against every test that rewrites the process environment (writers).
///
/// `serial_test` keys only exclude tests that carry the same key, so a test with no key still reads `DATABASE_URL`, `QUEUE_URL` and the rest while it boots.
/// A writer therefore waits for the booting tests in flight and holds the others off until its [`EnvGuard`] is dropped.
///
/// A read skips lock acquisition only inside a write scope on the same thread.
/// Nested environment guards share that write lock until the outer guard exits.
/// A write inside a read is a bug in the test: it would wait on itself, so it is refused loudly.
/// Capture the [`EnvGuard`] before the call that boots the application.
static ENVIRONMENT: std::sync::RwLock<()> = std::sync::RwLock::new(());

thread_local! {
    static SCOPES: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
}

/// Held for as long as a test reads the process environment while it runs.
pub(crate) struct EnvironmentRead {
    _lock: Option<std::sync::RwLockReadGuard<'static, ()>>,
}

impl EnvironmentRead {
    pub(crate) fn enter() -> Self {
        let held = SCOPES.with(|scopes| scopes.get().1 > 0);
        let lock = (!held).then(|| ENVIRONMENT.read().unwrap_or_else(|e| e.into_inner()));
        SCOPES.with(|scopes| {
            let (readers, writers) = scopes.get();
            scopes.set((readers + 1, writers));
        });
        Self { _lock: lock }
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
    assert!(read._lock.is_none(), "the writer already owns the lock");
}

#[test]
fn an_environment_write_inside_a_reader_is_refused() {
    let _read = EnvironmentRead::enter();
    let result = std::panic::catch_unwind(EnvironmentWrite::enter);
    assert!(result.is_err(), "a write cannot bypass a held read lock");
    assert_eq!(SCOPES.with(std::cell::Cell::get), (1, 0));
}

impl EnvironmentWrite {
    fn enter() -> Self {
        let (readers, writers) = SCOPES.with(std::cell::Cell::get);
        debug_assert!(
            readers == 0 || writers > 0,
            "an EnvGuard was captured inside a booted test's scope: capture it before boot_request"
        );
        let lock = (readers == 0 && writers == 0)
            .then(|| ENVIRONMENT.write().unwrap_or_else(|e| e.into_inner()));
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
                .map(|variable| (*variable, env::var_os(variable)))
                .collect(),
        }
    }

    pub(crate) fn set(&self, variable: &'static str, value: impl AsRef<OsStr>) {
        assert!(
            self.values.iter().any(|(name, _)| *name == variable),
            "environment variable {variable} was not captured"
        );
        // SAFETY: callers hold the `process_environment` serial_test lock while this guard is live.
        unsafe { env::set_var(variable, value) };
    }

    pub(crate) fn remove(&self, variable: &'static str) {
        assert!(
            self.values.iter().any(|(name, _)| *name == variable),
            "environment variable {variable} was not captured"
        );
        // SAFETY: callers hold the `process_environment` serial_test lock while this guard is live.
        unsafe { env::remove_var(variable) };
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: the guard's test owns the `process_environment` serial_test lock until Drop.
        unsafe {
            for (variable, original) in &self.values {
                match original {
                    Some(value) => env::set_var(variable, value),
                    None => env::remove_var(variable),
                }
            }
        }
    }
}

#[test]
#[serial(process_environment)]
fn env_guard_restores_original_value_on_normal_drop() {
    const VARIABLE: &str = "YORISHIRO_TEST_ENV_GUARD";
    let outer = EnvGuard::capture(&[VARIABLE]);
    let original = OsString::from("original-value");
    outer.set(VARIABLE, &original);

    {
        let guard = EnvGuard::capture(&[VARIABLE]);
        guard.set(VARIABLE, "temporary-value");
    }

    assert_eq!(env::var_os(VARIABLE), Some(original));
}

#[test]
#[serial(process_environment)]
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
    assert_eq!(env::var_os(VARIABLE), Some(original));
}

/// Records whether a backend-specific test was selected for this test job.
fn require_backend(expected: &str) -> bool {
    let url = std::env::var("DATABASE_URL").unwrap_or_default();
    let actual = if url.starts_with("sqlite:") {
        "sqlite"
    } else if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        "postgres"
    } else {
        "unknown"
    };
    let outcome = if actual == expected {
        "executed"
    } else {
        "skipped"
    };

    let reason = if outcome == "executed" {
        format!("selected {expected}-specific test on {actual} backend")
    } else {
        format!("skipped {expected}-specific test: active backend is {actual}")
    };

    eprintln!("{reason}");
    if let Ok(root) = std::env::var("YORISHIRO_BACKEND_MARKER_DIR") {
        let directory = std::path::Path::new(&root).join(expected).join(outcome);
        fs::create_dir_all(&directory).expect("create backend test marker directory");
        let sequence = BACKEND_MARKER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let marker = directory.join(format!("{}-{sequence}", std::process::id()));
        fs::write(marker, format!("{reason}\n")).expect("write backend test marker");
    }

    actual == expected
}

/// Returns `true` and records execution only when the active backend is SQLite.
pub(crate) fn require_sqlite_backend() -> bool {
    require_backend("sqlite")
}

/// Returns `true` and records execution only when the active backend is PostgreSQL.
pub(crate) fn require_postgres_backend() -> bool {
    require_backend("postgres")
}

/// The reserved Valkey test database (`/15`) this lane's queue points at, or `None` when the lane has no Valkey queue.
///
/// `YORISHIRO_VALKEY_TEST_URL` names it explicitly; otherwise a Redis `QUEUE_URL` is the lane's own.
pub(crate) fn valkey_test_url() -> Option<String> {
    let uri = std::env::var("YORISHIRO_VALKEY_TEST_URL")
        .or_else(|_| std::env::var("QUEUE_URL"))
        .ok()?;
    let reserved = (uri.starts_with("redis://") || uri.starts_with("rediss://"))
        && reqwest::Url::parse(&uri).is_ok_and(|url| url.path() == "/15");
    reserved.then_some(uri)
}
