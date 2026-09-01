mod licence;
mod metaschema;
mod migration;
mod models;
mod requests;
mod services;
mod tasks;

/// Returns `true` when `DATABASE_URL` points to SQLite, `false` otherwise.
/// Callers should do `if !require_sqlite_backend() { return; }` to skip
/// without terminating the test binary (a bare `process::exit` kills every
/// other test in the process).
pub(crate) fn require_sqlite_backend() -> bool {
    let url = std::env::var("DATABASE_URL").unwrap_or_default();
    if !url.starts_with("sqlite://") && !url.starts_with("sqlite::memory:") {
        eprintln!(
            "skipping SQLite-only test (DATABASE_URL={} is not SQLite)",
            url
        );
        false
    } else {
        true
    }
}

/// Returns `true` when `YORISHIRO_TEST_BACKEND` is unset or matches `expected`.
/// Used by CI matrix entries so that backend-gated tests only run on their own
/// backend; unset means run everything (local `cargo test` default).
/// Callers should do `if !require_test_backend("sqlite") { return; }` to skip
/// without terminating the test binary.
pub(crate) fn require_test_backend(expected: &str) -> bool {
    let backend = std::env::var("YORISHIRO_TEST_BACKEND").unwrap_or_default();
    if !backend.is_empty() && backend != expected {
        eprintln!(
            "skipping {} test (YORISHIRO_TEST_BACKEND={} targets {})",
            expected, backend, expected
        );
        false
    } else {
        true
    }
}
