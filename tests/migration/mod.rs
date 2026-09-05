//! Migration tests: apply/roll-back/reapply on both SQLite and PostgreSQL.
//!
//! Integration tests for the migration crate.
//! After folding the migration crate into the root package, they live here under
//! `tests/migration/` so Cargo still recognizes them as integration tests.

mod postgres;
mod sqlite;
