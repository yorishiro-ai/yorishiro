//! Every migration must produce a valid schema on SQLite (single-tenant, no RLS/roles/vector/trgm — see `helpers.rs`), not just on Postgres.

use std::sync::atomic::{AtomicU32, Ordering};

use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database};
use serial_test::serial;

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
#[serial(process_environment)]
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
#[serial(process_environment)]
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
    // Exercises every down()'s pg_only/sqlite_only guards (schema_schemas' DROP TRIGGER/FUNCTION and its own trigger drop, both authenticate_api_key files), not just up()'s.
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
#[serial(process_environment)]
async fn migration_sqlite_max_connections_10_five_times() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let dir = tempfile::tempdir().expect("create tempdir");
    for _ in 0..5 {
        let path = sqlite_path(dir.path());
        let mut options =
            sea_orm::ConnectOptions::new(format!("sqlite://{}?mode=rwc", path.display()));
        options.max_connections(10).min_connections(2);
        let db = Database::connect(options).await.expect("connect sqlite");

        Migrator::up(&db, None).await.expect("migration failed");

        drop(db);
        cleanup(&path);
    }
}

/// A database upgraded from the previous migration must replace the old audit action CHECK,
/// rather than only accepting `fill_defaults` on a fresh install.
#[tokio::test]
#[serial(process_environment)]
async fn fill_defaults_audit_action_upgrades_an_existing_sqlite_schema() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir.path().join("yorishiro_migrate_upgrade.sqlite3");
    let db = Database::connect(format!("sqlite://{}?mode=rwc", db_path.display()))
        .await
        .expect("connect to sqlite file");

    Migrator::up(&db, Some(5))
        .await
        .expect("apply migrations through the old schema");
    db.execute_unprepared("PRAGMA foreign_keys = OFF;")
        .await
        .expect("disable foreign keys for check-only insert");
    let old_insert = db
        .execute_unprepared(
            "INSERT INTO api_key_audit_log (id, workspace_id, tenant_id, action, detail) \
             VALUES (randomblob(16), randomblob(16), randomblob(16), 'fill_defaults', '{}')",
        )
        .await;
    assert!(
        old_insert.is_err(),
        "the old CHECK must reject fill_defaults"
    );

    Migrator::up(&db, None)
        .await
        .expect("apply the forward audit CHECK migration");
    db.execute_unprepared(
        "INSERT INTO api_key_audit_log (id, workspace_id, tenant_id, action, detail) \
         VALUES (randomblob(16), randomblob(16), randomblob(16), 'fill_defaults', '{}')",
    )
    .await
    .expect("the upgraded CHECK must accept fill_defaults");
}

/// The query embedding table is created by migration 000018 and dropped when it rolls back.
#[tokio::test]
#[serial(process_environment)]
async fn query_embedding_requests_migration_is_reversible_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let dir = tempfile::tempdir().expect("create tempdir");
    let url = format!(
        "sqlite://{}?mode=rwc",
        dir.path().join("qe.sqlite3").display()
    );
    let db = Database::connect(&url).await.expect("connect");
    let exists = |db: &sea_orm::DatabaseConnection| {
        let db = db.clone();
        async move {
            db.query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'query_embedding_requests'",
            ))
            .await
            .expect("read schema")
            .is_some()
        }
    };

    Migrator::up(&db, None).await.expect("up");
    assert!(exists(&db).await);
    let position = Migrator::migrations()
        .iter()
        .position(|migration| migration.name() == "m20261006_000018_embedding_queue_state")
        .expect("migration 000018 is registered");
    let later = Migrator::migrations().len() - position;
    Migrator::down(&db, Some(later as u32))
        .await
        .expect("roll back to before the query embedding table");
    assert!(!exists(&db).await);
    Migrator::up(&db, None).await.expect("up again");
    assert!(exists(&db).await);
}

#[tokio::test]
#[serial(process_environment)]
async fn queue_epoch_upgrade_preserves_and_quarantines_existing_outbox_rows() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    use sea_orm::{ConnectionTrait, Statement, TransactionTrait};
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Database::connect(format!(
        "sqlite://{}?mode=rwc",
        dir.path().join("outbox-upgrade.sqlite3").display()
    ))
    .await
    .expect("connect");
    Migrator::up(&db, None).await.expect("all migrations");
    Migrator::down(&db, Some(3))
        .await
        .expect("roll back snapshot, publisher and deadline migrations");
    db.execute_unprepared("DROP TABLE queue_job_dispatch_outbox; CREATE TABLE queue_job_dispatch_outbox (lifecycle_id BLOB PRIMARY KEY, payload TEXT NOT NULL, worker_name TEXT NOT NULL, queue_name TEXT, tags TEXT, last_attempt_at TEXT); INSERT INTO queue_job_dispatch_outbox VALUES (NULL, '{\"legacy\":true}', 'OldWorker', NULL, NULL, NULL)").await.expect("create and seed previous outbox schema");
    db.execute_unprepared("INSERT INTO queue_job_lifecycles (id, job_name, worker_class, workspace_id, status, attempt, enqueue_at, created_at, updated_at) VALUES (randomblob(16), 'upgrade_probe', 'shared', randomblob(16), 'queued', 1, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)").await.expect("seed due lifecycle");
    db.execute_unprepared("INSERT INTO queue_job_dispatch_outbox (lifecycle_id, payload, worker_name, queue_name, tags, last_attempt_at) SELECT id, '{\"upgrade\":true}', 'UpgradeWorker', NULL, NULL, NULL FROM queue_job_lifecycles WHERE job_name = 'upgrade_probe'").await.expect("seed matching old outbox");
    let migration = Migrator::migrations()
        .into_iter()
        .find(|m| m.name() == "m20261015_160234_queue_publishing_epoch")
        .unwrap();
    migration
        .up(&migration::SchemaManager::new(&db))
        .await
        .expect("upgrade outbox");
    let snapshot_migration = Migrator::migrations()
        .into_iter()
        .find(|m| m.name() == "m20261016_000021_queue_replay_snapshot_id")
        .unwrap();
    snapshot_migration
        .up(&migration::SchemaManager::new(&db))
        .await
        .expect("upgrade replay snapshot state");
    let upgraded = db
        .query_one_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            "SELECT legacy_quarantined FROM queue_job_dispatch_outbox",
        ))
        .await
        .expect("read upgraded row")
        .expect("upgraded row preserved");
    assert!(
        upgraded.try_get::<bool>("", "legacy_quarantined").unwrap(),
        "legacy rows without safe scope must be quarantined"
    );
    let due = yorishiro::models::queue_job_dispatch_outbox::due(&db)
        .await
        .expect("decode due upgraded dispatches");
    let due = due
        .into_iter()
        .find(|dispatch| dispatch.worker_name == "UpgradeWorker")
        .expect("upgraded dispatch remains due");
    assert_eq!(due.id, due.lifecycle_id.expect("lifecycle id"));
    let txn = db.begin().await.expect("begin replay upgrade transaction");
    yorishiro::models::queue_job_dispatch_outbox::advance_publishing_epoch(&txn, 1, false, None)
        .await
        .expect("pause publisher");
    let replayed = yorishiro::models::queue_job_dispatch_outbox::prepare_replay_snapshot(
        &txn,
        2,
        chrono::Utc::now().fixed_offset(),
        true,
        "sqlite-upgrade-evidence",
    )
    .await
    .expect("requeue upgraded dispatch");
    txn.commit().await.expect("commit replay upgrade");
    assert_eq!(replayed, 1);
    migration
        .down(&migration::SchemaManager::new(&db))
        .await
        .expect("rollback epoch schema");
    let row = db
        .query_one_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            "SELECT payload, worker_name FROM queue_job_dispatch_outbox WHERE lifecycle_id IS NULL",
        ))
        .await
        .expect("read legacy row")
        .expect("legacy row preserved");
    assert_eq!(
        row.try_get::<String>("", "worker_name").unwrap(),
        "OldWorker"
    );
}
