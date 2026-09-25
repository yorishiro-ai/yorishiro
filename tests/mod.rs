mod config;
mod licence;
mod metaschema;
mod migration;
mod models;
mod requests;
mod services;
mod tasks;
mod workers;

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serial_test::serial;

static BACKEND_MARKER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Captures process environment values and restores them when the test exits.
///
/// The guard is intentionally Drop-based so restoration also runs while a test unwinds from a
/// panic.
pub(crate) struct EnvGuard {
    values: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    pub(crate) fn capture(variables: &[&'static str]) -> Self {
        Self {
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

pub(crate) struct CurrentDirGuard {
    original: PathBuf,
}

impl CurrentDirGuard {
    pub(crate) fn enter(path: &Path) -> Self {
        let original = env::current_dir().unwrap();
        env::set_current_dir(path).unwrap();
        Self { original }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        env::set_current_dir(&self.original).unwrap();
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
    let actual = if url.starts_with("sqlite://") || url.starts_with("sqlite::memory:") {
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
