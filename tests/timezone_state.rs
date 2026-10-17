use migration::MigratorTrait;
use sea_orm::{ConnectionTrait, EntityTrait, Statement, TransactionTrait};
use serial_test::serial;
use std::{sync::Arc, time::Duration};
use tokio::sync::Barrier;

#[tokio::test]
async fn sqlite_timezone_state_is_seeded_noop_safe_and_preserves_instants() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let db = sqlite_database().await;
    let lifecycle_id = uuid::Uuid::now_v7();
    yorishiro::models::queue_job_lifecycles::Entity::record_enqueue(
        &db,
        yorishiro::models::queue_job_lifecycles::Enqueue {
            id: lifecycle_id,
            job_name: "time-test",
            worker_class: yorishiro::workers::embedding_sync::WorkerClass::Shared,
            workspace_id: None,
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
    )
    .await
    .unwrap();
    let before = yorishiro::models::queue_job_lifecycles::Entity::find_by_id(lifecycle_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();

    db.execute_unprepared("CREATE TABLE timezone_state_audit (at TEXT NOT NULL)")
        .await
        .unwrap();

    yorishiro::models::timezone_state::activate(&db, "UTC")
        .await
        .unwrap();
    db.execute_unprepared(
        "CREATE TRIGGER audit_zone_update AFTER UPDATE ON timezone_state BEGIN INSERT INTO timezone_state_audit VALUES (CURRENT_TIMESTAMP); END",
    )
    .await
    .unwrap();
    let no_op_updates: i64 = db
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT count(*) FROM timezone_state_audit",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index(0)
        .unwrap();
    assert_eq!(
        no_op_updates, 0,
        "same-zone activation must issue no UPDATE"
    );
    yorishiro::models::timezone_state::activate(&db, "UTC")
        .await
        .unwrap();
    yorishiro::models::timezone_state::activate(&db, "Asia/Tokyo")
        .await
        .unwrap();
    yorishiro::models::timezone_state::activate(&db, "America/New_York")
        .await
        .unwrap();
    yorishiro::models::timezone_state::activate(&db, "Europe/Paris")
        .await
        .unwrap();
    let after = yorishiro::models::queue_job_lifecycles::Entity::find_by_id(lifecycle_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.enqueue_at, after.enqueue_at);
    let row = db
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT active_timezone FROM timezone_state WHERE id = 1",
        ))
        .await
        .unwrap()
        .unwrap();
    let name: String = row.try_get_by_index(0).unwrap();
    assert_eq!(name, "Europe/Paris");
}

#[tokio::test]
async fn sqlite_timezone_state_serializes_concurrent_activations() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let db = sqlite_database().await;
    let barrier = Arc::new(Barrier::new(3));
    let first = db.clone();
    let first_barrier = barrier.clone();
    let a = tokio::spawn(async move {
        first_barrier.wait().await;
        yorishiro::models::timezone_state::activate(&first, "Asia/Tokyo").await
    });
    let second = db.clone();
    let second_barrier = barrier.clone();
    let b = tokio::spawn(async move {
        second_barrier.wait().await;
        yorishiro::models::timezone_state::activate(&second, "America/New_York").await
    });
    barrier.wait().await;
    let (a, b) = tokio::time::timeout(Duration::from_secs(8), async { (a.await, b.await) })
        .await
        .expect("concurrent activation must finish within its lock timeout");
    a.unwrap().unwrap();
    b.unwrap().unwrap();

    let row = db
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT active_timezone FROM timezone_state WHERE id = 1",
        ))
        .await
        .unwrap()
        .unwrap();
    let name: String = row.try_get_by_index(0).unwrap();
    assert!(name == "Asia/Tokyo" || name == "America/New_York");
}

#[tokio::test]
async fn sqlite_timezone_state_serializes_writers_on_separate_connections() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let path = tempfile::tempdir()
        .unwrap()
        .keep()
        .join("timezone-state-race.sqlite3");
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let db = sqlite_connection(&url).await;
    migration::Migrator::up(&db, None).await.unwrap();
    let audit_db = sqlite_connection(&url).await;
    let initial: String = audit_db
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT active_timezone FROM timezone_state WHERE id = 1",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index(0)
        .unwrap();
    assert_eq!(initial, "UTC");
    audit_db
        .execute_unprepared("CREATE TABLE timezone_state_update_audit (zone TEXT NOT NULL)")
        .await
        .unwrap();
    audit_db
        .execute_unprepared(
            "CREATE TRIGGER audit_timezone_state_update AFTER UPDATE ON timezone_state BEGIN INSERT INTO timezone_state_update_audit VALUES (NEW.active_timezone); END",
        )
        .await
        .unwrap();
    let first = sqlite_connection(&url).await;
    let second = sqlite_connection(&url).await;
    let first_task = tokio::spawn(async move {
        yorishiro::models::timezone_state::activate(&first, "Asia/Tokyo").await
    });
    let second_task = tokio::spawn(async move {
        yorishiro::models::timezone_state::activate(&second, "America/New_York").await
    });
    for task in [first_task, second_task] {
        tokio::time::timeout(Duration::from_secs(15), task)
            .await
            .expect("separate SQLite writer finishes")
            .unwrap()
            .unwrap();
    }
    let zone = db
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT active_timezone FROM timezone_state WHERE id = 1",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index::<String>(0)
        .unwrap();
    assert!(zone == "Asia/Tokyo" || zone == "America/New_York");
    audit_db
        .execute_unprepared("DELETE FROM timezone_state_update_audit")
        .await
        .unwrap();
    let same_zone = sqlite_connection(&url).await;
    yorishiro::models::timezone_state::activate(&same_zone, &zone)
        .await
        .unwrap();
    let updates = audit_db
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT count(*) FROM timezone_state_update_audit",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index::<i64>(0)
        .unwrap();
    assert_eq!(updates, 0, "same-zone activation must issue no UPDATE");
    audit_db
        .execute_unprepared("DROP TRIGGER audit_timezone_state_update")
        .await
        .unwrap();
    audit_db
        .execute_unprepared("DROP TABLE timezone_state_update_audit")
        .await
        .unwrap();
}

#[tokio::test]
async fn failed_activation_rolls_back_the_singleton_update() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let db = sqlite_database().await;
    let transaction = db.begin().await.unwrap();
    transaction
        .execute_unprepared(
            "CREATE TRIGGER reject_zone_update BEFORE UPDATE ON timezone_state BEGIN SELECT RAISE(ABORT, 'test rollback'); END",
        )
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    assert!(
        yorishiro::models::timezone_state::activate(&db, "Asia/Tokyo")
            .await
            .is_err()
    );
    let row = db
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT active_timezone FROM timezone_state WHERE id = 1",
        ))
        .await
        .unwrap()
        .unwrap();
    let name: String = row.try_get_by_index(0).unwrap();
    assert_eq!(name, "UTC");
    db.execute_unprepared("DROP TRIGGER reject_zone_update")
        .await
        .unwrap();
    yorishiro::models::timezone_state::activate(&db, "Asia/Tokyo")
        .await
        .unwrap();
    let retry_zone = db
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT active_timezone FROM timezone_state WHERE id = 1",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index::<String>(0)
        .unwrap();
    assert_eq!(retry_zone, "Asia/Tokyo");
    yorishiro::models::timezone_state::activate(&db, "Asia/Tokyo")
        .await
        .unwrap();
}

#[tokio::test]
#[serial(postgres_cluster)]
async fn postgres_timezone_state_serializes_concurrent_activations_without_dml_noop() {
    if !crate::require_postgres_backend() {
        return;
    }
    crate::require_sqlite_backend();
    crate::migration::postgres::with_timezone_scratch_db(|db, database_url| {
        Box::pin(async move {
            postgres_timezone_state_scenario(db, database_url)
                .await
                .unwrap()
        })
    })
    .await;
}

async fn postgres_timezone_state_scenario(
    db: &sea_orm::DatabaseConnection,
    database_url: &str,
) -> Result<(), String> {
    migration::Migrator::up(db, None)
        .await
        .map_err(|error| error.to_string())?;
    db.execute_unprepared(
        "CREATE TABLE timezone_state_test_probe (id integer PRIMARY KEY, at timestamptz NOT NULL)",
    )
    .await
    .map_err(|error| error.to_string())?;
    db.execute_unprepared(
        "INSERT INTO timezone_state_test_probe VALUES (1, '2026-10-16 00:00:00+00')",
    )
    .await
    .map_err(|error| error.to_string())?;
    let mut timestamp_options = sea_orm::ConnectOptions::new(database_url.to_owned());
    timestamp_options.max_connections(1).min_connections(1);
    let timestamp_connection = sea_orm::Database::connect(timestamp_options)
        .await
        .map_err(|error| error.to_string())?;
    timestamp_connection
        .execute_unprepared("SET TIME ZONE 'UTC'")
        .await
        .map_err(|error| error.to_string())?;
    db.execute_unprepared("CREATE TABLE timezone_state_test_audit (at timestamptz NOT NULL)")
        .await
        .map_err(|error| error.to_string())?;
    let before = read_zone(db).await;
    let before_instant = read_probe_instant(db).await;
    yorishiro::db::set_session_timezone(&timestamp_connection, "Asia/Tokyo")
        .await
        .map_err(|error| error.to_string())?;
    let tokyo_rendered = timestamp_connection
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT at::text FROM timezone_state_test_probe WHERE id = 1",
        ))
        .await
        .map_err(|error| error.to_string())?
        .unwrap()
        .try_get_by_index::<String>(0)
        .unwrap();
    assert!(tokyo_rendered.ends_with("+09"), "{tokyo_rendered}");
    yorishiro::db::set_session_timezone(&timestamp_connection, "America/New_York")
        .await
        .map_err(|error| error.to_string())?;
    let new_york_rendered = timestamp_connection
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT at::text FROM timezone_state_test_probe WHERE id = 1",
        ))
        .await
        .map_err(|error| error.to_string())?
        .unwrap()
        .try_get_by_index::<String>(0)
        .unwrap();
    assert!(new_york_rendered.ends_with("-04"), "{new_york_rendered}");
    assert_eq!(read_probe_instant(db).await, before_instant);
    timestamp_connection
        .close()
        .await
        .map_err(|error| error.to_string())?;
    db.execute_unprepared("CREATE FUNCTION audit_zone_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO timezone_state_test_audit VALUES (now()); RETURN NEW; END $$")
        .await.map_err(|error| error.to_string())?;
    db.execute_unprepared("CREATE TRIGGER audit_zone_update AFTER UPDATE ON timezone_state FOR EACH ROW EXECUTE FUNCTION audit_zone_update()")
        .await.map_err(|error| error.to_string())?;
    yorishiro::models::timezone_state::activate(db, &before)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(read_probe_instant(db).await, before_instant);
    let retry_count: i64 = db
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT count(*) FROM timezone_state_test_audit",
        ))
        .await
        .map_err(|error| error.to_string())?
        .unwrap()
        .try_get_by_index(0)
        .unwrap();
    assert_eq!(retry_count, 0, "same-zone activation must issue no UPDATE");
    yorishiro::models::timezone_state::activate(db, "Asia/Tokyo")
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(read_probe_instant(db).await, before_instant);
    yorishiro::models::timezone_state::activate(db, "America/New_York")
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(read_probe_instant(db).await, before_instant);
    yorishiro::models::timezone_state::activate(db, "Europe/Paris")
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(read_probe_instant(db).await, before_instant);
    let barrier = Arc::new(Barrier::new(3));
    let left = db.clone();
    let left_barrier = barrier.clone();
    let a = tokio::spawn(async move {
        left_barrier.wait().await;
        yorishiro::models::timezone_state::activate(&left, "Asia/Tokyo").await
    });
    let right = db.clone();
    let right_barrier = barrier.clone();
    let b = tokio::spawn(async move {
        right_barrier.wait().await;
        yorishiro::models::timezone_state::activate(&right, "America/New_York").await
    });
    barrier.wait().await;
    let (a, b) = tokio::time::timeout(Duration::from_secs(10), async { (a.await, b.await) })
        .await
        .map_err(|error| error.to_string())?;
    a.map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    b.map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    assert!(matches!(
        read_zone(db).await.as_str(),
        "Asia/Tokyo" | "America/New_York"
    ));
    assert_eq!(read_probe_instant(db).await, before_instant);
    db.execute_unprepared("CREATE FUNCTION reject_timezone_state_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test rollback'; END $$")
        .await.map_err(|error| error.to_string())?;
    db.execute_unprepared("CREATE TRIGGER reject_timezone_state_update BEFORE UPDATE ON timezone_state FOR EACH ROW EXECUTE FUNCTION reject_timezone_state_update()")
        .await.map_err(|error| error.to_string())?;
    let original = read_zone(db).await;
    assert!(
        yorishiro::models::timezone_state::activate(db, "Europe/Paris")
            .await
            .is_err()
    );
    assert_eq!(read_zone(db).await, original);
    let audit_count: i64 = db
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT count(*) FROM timezone_state_test_audit",
        ))
        .await
        .map_err(|error| error.to_string())?
        .unwrap()
        .try_get_by_index(0)
        .unwrap();
    assert_eq!(
        audit_count, 5,
        "failed update must not fire the AFTER trigger"
    );
    db.execute_unprepared(
        "DROP TRIGGER reject_timezone_state_update ON timezone_state; DROP FUNCTION reject_timezone_state_update()",
    )
    .await
    .map_err(|error| error.to_string())?;
    yorishiro::models::timezone_state::activate(db, "Europe/Paris")
        .await
        .map_err(|error| error.to_string())?;
    let audit_count: i64 = db
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT count(*) FROM timezone_state_test_audit",
        ))
        .await
        .map_err(|error| error.to_string())?
        .unwrap()
        .try_get_by_index(0)
        .unwrap();
    assert_eq!(
        audit_count, 6,
        "zone transitions plus retry should issue six updates"
    );
    yorishiro::models::timezone_state::activate(db, "Europe/Paris")
        .await
        .map_err(|error| error.to_string())?;
    let retry_count: i64 = db
        .query_one_raw(Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT count(*) FROM timezone_state_test_audit",
        ))
        .await
        .map_err(|error| error.to_string())?
        .unwrap()
        .try_get_by_index(0)
        .unwrap();
    assert_eq!(retry_count, 6);
    Ok(())
}

async fn read_zone(db: &sea_orm::DatabaseConnection) -> String {
    use sea_orm::ConnectionTrait;

    db.query_one_raw(Statement::from_string(
        sea_orm::DbBackend::Postgres,
        "SELECT active_timezone FROM timezone_state WHERE id = 1",
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get_by_index(0)
    .unwrap()
}

async fn read_probe_instant(db: &sea_orm::DatabaseConnection) -> chrono::DateTime<chrono::Utc> {
    db.query_one_raw(Statement::from_string(
        sea_orm::DbBackend::Postgres,
        "SELECT at FROM timezone_state_test_probe WHERE id = 1",
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get_by_index(0)
    .unwrap()
}

async fn sqlite_database() -> sea_orm::DatabaseConnection {
    use sea_orm::Database;

    let path = tempfile::tempdir()
        .unwrap()
        .keep()
        .join("timezone-state.sqlite3");
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let mut options = sea_orm::ConnectOptions::new(url);
    options.map_sqlx_sqlite_opts(|opts| opts.busy_timeout(Duration::from_secs(5)));
    options.max_connections(2);
    let db = Database::connect(options).await.unwrap();
    migration::Migrator::up(&db, None).await.unwrap();
    db
}

async fn sqlite_connection(url: &str) -> sea_orm::DatabaseConnection {
    use sea_orm::Database;

    let mut options = sea_orm::ConnectOptions::new(url.to_owned());
    options.max_connections(2);
    options.map_sqlx_sqlite_opts(|opts| opts.busy_timeout(Duration::from_secs(5)));
    Database::connect(options).await.unwrap()
}
