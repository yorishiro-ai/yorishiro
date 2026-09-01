//! Migration tests: apply/roll-back/reapply on both SQLite and PostgreSQL.
//!
//! These were originally in `migration/tests/` as part of the `migration/` crate.
//! After folding the migration crate into the root package, they live here under
//! `tests/migration/` so Cargo still recognizes them as integration tests.

// Re-export Migrator and MigratorTrait so the test files can use `crate::migration::...`
// instead of `yorishiro::migration::...`.
pub use yorishiro::migration::{Migrator, MigratorTrait};

mod postgres;
mod sqlite;

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
