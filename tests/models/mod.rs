mod content_entities_sqlite;
mod entities;
mod identity_api_keys;
mod identity_templates;
mod recall;
mod search;
mod tenancy;
mod tenancy_sqlite;

/// Skip when `DATABASE_URL` is not a SQLite URL, so these tests do not run in the
/// postgres-only CI matrix entry. Local `cargo test` (where DATABASE_URL defaults to
/// postgres) also skips them; set `DATABASE_URL=sqlite::memory:` to run them locally.
pub(crate) fn require_sqlite_backend() {
    let url = std::env::var("DATABASE_URL").unwrap_or_default();
    if !url.starts_with("sqlite://") && !url.starts_with("sqlite::memory:") {
        eprintln!(
            "skipping SQLite-only test (DATABASE_URL={} is not SQLite)",
            url
        );
        std::process::exit(0);
    }
}

/// Skip when `DATABASE_URL` is not a SQLite URL, so these tests do not run in the
/// postgres-only CI matrix entry. Local `cargo test` (where DATABASE_URL defaults to
/// postgres) also skips them; set `DATABASE_URL=sqlite::memory:` to run them locally.
pub(crate) fn require_sqlite_backend() {
    let url = std::env::var("DATABASE_URL").unwrap_or_default();
    if !url.starts_with("sqlite://") && !url.starts_with("sqlite::memory:") {
        eprintln!(
            "skipping SQLite-only test (DATABASE_URL={} is not SQLite)",
            url
        );
        std::process::exit(0);
    }
}
