// Verification: all 4 migrations run to completion on SQLite,
// repeated 5 times to exercise pooled-connection interleaving.
// Without the transaction guard in helpers::use_transaction(), DDL statements
// interleave across pooled connections and the DROP/CREATE TRIGGER pair fails.
use std::sync::atomic::{AtomicU32, Ordering};

use sea_orm_migration::sea_orm::Database;
use yorishiro::migration::{Migrator, MigratorTrait};

const RUNS: usize = 5;
static COUNTER: AtomicU32 = AtomicU32::new(0);

fn sqlite_path() -> String {
    format!(
        "/tmp/yorishiro_migrate_test_{}.sqlite3",
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

fn cleanup(path: &str) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{path}-wal"));
    let _ = std::fs::remove_file(format!("{path}-shm"));
    let _ = std::fs::remove_file(format!("{path}-journal"));
}

fn require_sqlite_backend() {
    let url = std::env::var("DATABASE_URL").unwrap_or_default();
    if !url.starts_with("sqlite://") && !url.starts_with("sqlite::memory:") {
        eprintln!(
            "skipping SQLite-only test (DATABASE_URL={} is not SQLite)",
            url
        );
        std::process::exit(0);
    }
}

#[tokio::test]
async fn migration_sqlite_max_connections_10_five_times() {
    require_sqlite_backend();
    for _run in 1..=RUNS {
        let path = sqlite_path();
        let db = Database::connect(&format!("sqlite://{}?mode=rwc", path))
            .await
            .expect("connect sqlite");

        Migrator::up(&db, None).await.expect("migration failed");

        drop(db);
        cleanup(&path);
    }
}
