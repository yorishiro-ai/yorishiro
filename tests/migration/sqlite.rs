//! Every migration must produce a valid schema on SQLite (single-tenant, no RLS/roles/vector/trgm — see `helpers.rs`), not just on Postgres.

use std::sync::atomic::{AtomicU32, Ordering};

use sea_orm_migration::sea_orm::Database;

use crate::migration::{Migrator, MigratorTrait};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn sqlite_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(format!(
        "yorishiro_migrate_{}.sqlite3",
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    let _ = std::fs::remove_file(path.with_extension("sqlite3-journal"));
}

#[tokio::test]
async fn all_migrations_apply_to_a_fresh_sqlite_file() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir.path().join("yorishiro.sqlite3");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());

    let db = Database::connect(&url)
        .await
        .expect("connect to sqlite file");

    Migrator::up(&db, None).await.expect("run all migrations");
}

#[tokio::test]
async fn all_migrations_roll_back_and_reapply_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir.path().join("yorishiro.sqlite3");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());

    let db = Database::connect(&url)
        .await
        .expect("connect to sqlite file");

    Migrator::up(&db, None).await.expect("run all migrations");
    // Exercises every down()'s pg_only/sqlite_only guards (content_schemas' DROP TRIGGER/FUNCTION and its own trigger drop, both authenticate_api_key files), not just up()'s.
    Migrator::down(&db, None)
        .await
        .expect("roll back all migrations");
    Migrator::up(&db, None)
        .await
        .expect("reapply all migrations after rollback");
}

// Verification: all migrations run to completion on SQLite,
// repeated 5 times to exercise pooled-connection interleaving.
// Without the transaction guard in helpers::use_transaction(), DDL statements
// interleave across pooled connections and the DROP/CREATE TRIGGER pair fails.
#[tokio::test]
async fn migration_sqlite_max_connections_10_five_times() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let dir = tempfile::tempdir().expect("create tempdir");
    for _ in 0..5 {
        let path = sqlite_path(dir.path());
        let db = Database::connect(&format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .expect("connect sqlite");

        Migrator::up(&db, None).await.expect("migration failed");

        drop(db);
        cleanup(&path);
    }
}
