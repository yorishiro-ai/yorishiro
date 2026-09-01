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
mod sqlite_tx;
