mod config;
mod licence;
mod metaschema;
mod migration;
mod models;
mod requests;
mod services;
mod tasks;
mod workers;

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

static BACKEND_MARKER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
