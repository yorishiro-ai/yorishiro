use chrono::Offset;
use futures::FutureExt;
use loco_rs::app::Hooks;
use migration::MigratorTrait;
use sea_orm::Database;
use sea_orm::{ConnectionTrait, EntityTrait, Statement, TransactionTrait};
use serial_test::serial;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{sync::Arc, time::Duration};
use tokio::sync::Barrier;

#[tokio::test]
async fn postgres_app_pool_uses_configured_timezone_on_new_and_released_connections() {
    if !crate::require_postgres_backend() {
        return;
    }
    crate::requests::boot_request_with_timezone::<yorishiro::App, _, _>(
        "Asia/Tokyo",
        |_request, ctx| async move {
            let pool = ctx.db.get_postgres_connection_pool();
            let mut first = pool.acquire().await.unwrap();
            let mut second = pool.acquire().await.unwrap();
            assert_ne!(first.as_mut() as *mut _, second.as_mut() as *mut _);
            for connection in [&mut first, &mut second] {
                let zone: String = sea_orm::sqlx::query_scalar("SHOW TIME ZONE")
                    .fetch_one(&mut **connection)
                    .await
                    .unwrap();
                assert_eq!(zone, "Asia/Tokyo");
            }

            sea_orm::sqlx::query("SET TIME ZONE 'UTC'")
                .execute(&mut *first)
                .await
                .unwrap();
            drop(first);
            let mut reused = pool.acquire().await.unwrap();
            let zone: String = sea_orm::sqlx::query_scalar("SHOW TIME ZONE")
                .fetch_one(&mut *reused)
                .await
                .unwrap();
            assert_eq!(zone, "Asia/Tokyo");
            drop(reused);
            drop(second);
        },
    )
    .await;
}

#[tokio::test]
async fn postgres_pool_replacement_keeps_uri_settings_without_replaying_startup() {
    if !crate::require_postgres_backend() {
        return;
    }
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|_request, ctx| async move {
        let mut config = ctx.config.clone();
        let mut uri = reqwest::Url::parse(&config.database.uri).unwrap();
        uri.query_pairs_mut()
            .append_pair("options", "-c application_name=startup-session-test");
        config.database.uri = uri.into();
        config.database.run_on_start = Some("CREATE TABLE startup_probe AS SELECT 1 AS id".into());
        let replacement = loco_rs::boot::create_context::<yorishiro::App>(&ctx.environment, config)
            .await
            .unwrap();
        let handle = replacement
            .shared_store
            .get::<yorishiro::db::DbHandle>()
            .unwrap();
        assert!(Arc::ptr_eq(
            &replacement
                .db
                .get_postgres_connection_pool()
                .connect_options(),
            &handle.identity.connect_options()
        ));
        for pool in [&handle.identity, handle.tenant.pool()] {
            let mut first = pool.acquire().await.unwrap();
            let mut second = pool.acquire().await.unwrap();
            for connection in [&mut first, &mut second] {
                let name: String = sea_orm::sqlx::query_scalar("SHOW application_name")
                    .fetch_one(&mut **connection)
                    .await
                    .unwrap();
                assert_eq!(name, "startup-session-test");
            }
        }
        let count: i64 = sea_orm::sqlx::query_scalar("SELECT count(*) FROM startup_probe")
            .fetch_one(&handle.identity)
            .await
            .unwrap();
        assert_eq!(count, 1);
        crate::requests::close_app_pools(&replacement).await;
    })
    .await;
}

#[tokio::test]
async fn sqlite_timezone_state_is_seeded_noop_safe_and_preserves_instants() {
    if !crate::require_sqlite_backend() {
        return;
    }
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|_request, ctx| async move {
    let db = ctx.db.clone();
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
    })
    .await;
}

#[tokio::test]
async fn sqlite_current_timestamp_default_uses_configured_zone_on_each_boot() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory
        .path()
        .join(format!("timestamp-policy-{}.sqlite3", uuid::Uuid::now_v7()));
    let db_path = db_path.to_string_lossy().into_owned();
    let original = "2001-02-03 04:05:06";
    let legacy_utc = chrono::NaiveDateTime::parse_from_str(original, "%Y-%m-%d %H:%M:%S")
        .unwrap()
        .and_utc()
        .timestamp();
    for (index, zone) in ["Asia/Tokyo", "America/New_York", "UTC"]
        .into_iter()
        .enumerate()
    {
        let env = loco_rs::environment::Environment::Any("test_sqlite".into());
        let mut config = yorishiro::App::load_config(&env).await.unwrap();
        config.database.uri = format!("sqlite://{db_path}?mode=rwc");
        if let Some(loco_rs::config::QueueConfig::Sqlite(queue)) = config.queue.as_mut() {
            queue.uri = format!(
                "sqlite://{}?mode=rwc",
                directory.path().join("timestamp-queue.sqlite3").display()
            );
        }
        config.database.dangerously_truncate = false;
        config.database.dangerously_recreate = false;
        config.database.auto_migrate = false;
        config.settings.as_mut().unwrap()["timezone"] = zone.into();
        let ctx = loco_rs::boot::create_context::<yorishiro::App>(&env, config)
            .await
            .unwrap();
        let pool = ctx.db.get_sqlite_connection_pool();
        let mut first = pool.acquire().await.unwrap();
        let mut second = pool.acquire().await.unwrap();
        assert_ne!(first.as_mut() as *mut _, second.as_mut() as *mut _);
        for connection in [&mut first, &mut second] {
            let foreign_keys: i64 = sea_orm::sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(&mut **connection)
                .await
                .unwrap();
            let busy_timeout: i64 = sea_orm::sqlx::query_scalar("PRAGMA busy_timeout")
                .fetch_one(&mut **connection)
                .await
                .unwrap();
            assert_eq!((foreign_keys, busy_timeout), (1, 5000));
        }
        if index == 0 {
            sea_orm::sqlx::query(
                    "CREATE TABLE app_timestamp_probe (id INTEGER PRIMARY KEY, at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)",
                )
                .execute(&mut *first)
                .await
                .unwrap();
            sea_orm::sqlx::query("INSERT INTO app_timestamp_probe (at) VALUES (?)")
                .bind(original)
                .execute(&mut *first)
                .await
                .unwrap();
        }
        sea_orm::sqlx::query("INSERT INTO app_timestamp_probe DEFAULT VALUES")
            .execute(&mut *first)
            .await
            .unwrap();
        sea_orm::sqlx::query("INSERT INTO app_timestamp_probe DEFAULT VALUES")
            .execute(&mut *second)
            .await
            .unwrap();
        drop(first);
        drop(second);
        let rows: Vec<String> =
            sea_orm::sqlx::query_scalar("SELECT at FROM app_timestamp_probe ORDER BY id")
                .fetch_all(pool)
                .await
                .unwrap();
        assert_eq!(rows[0], original);
        let legacy_row = ctx
            .db
            .query_one_raw(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT at FROM app_timestamp_probe WHERE id = 1",
            ))
            .await
            .unwrap()
            .unwrap();
        let legacy_value: chrono::DateTime<chrono::FixedOffset> =
            legacy_row.try_get_by_index(0).unwrap();
        assert_eq!(legacy_value.timestamp(), legacy_utc);
        let latest = rows.last().unwrap();
        let parsed = chrono::DateTime::parse_from_rfc3339(latest).unwrap();
        let utc_now = chrono::Utc::now();
        let timezone = zone.parse::<chrono_tz::Tz>().unwrap();
        let expected = utc_now.with_timezone(&timezone);
        assert_eq!(
            parsed.timezone().local_minus_utc(),
            expected.offset().fix().local_minus_utc()
        );
        assert!((parsed.timestamp() - utc_now.timestamp()).abs() <= 2);
        let row = ctx
            .db
            .query_one_raw(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT at FROM app_timestamp_probe ORDER BY id DESC LIMIT 1",
            ))
            .await
            .unwrap()
            .unwrap();
        let seaorm_value: chrono::DateTime<chrono::FixedOffset> = row.try_get_by_index(0).unwrap();
        assert!((seaorm_value.timestamp() - utc_now.timestamp()).abs() <= 2);
        ctx.db.close().await.unwrap();
    }
}

#[tokio::test]
async fn sqlite_app_pool_executes_startup_effects_once_not_per_connection() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory
        .path()
        .join(format!("custom-pragmas-{}.sqlite3", uuid::Uuid::now_v7()));
    let env = loco_rs::environment::Environment::Any("test_sqlite".into());
    let mut config = yorishiro::App::load_config(&env).await.unwrap();
    config.database.uri = format!("sqlite://{}?mode=rwc", db_path.display());
    if let Some(loco_rs::config::QueueConfig::Sqlite(queue)) = config.queue.as_mut() {
        queue.uri = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("custom-queue.sqlite3").display()
        );
    }
    config.database.min_connections = 1;
    config.database.max_connections = 2;
    config.database.run_on_start = Some(
        "CREATE TABLE startup_probe (id INTEGER PRIMARY KEY); INSERT INTO startup_probe VALUES (1);"
            .to_owned(),
    );
    config.database.dangerously_truncate = false;
    config.database.dangerously_recreate = false;
    config.database.auto_migrate = false;
    config.settings.as_mut().unwrap()["timezone"] = "America/New_York".into();
    let ctx = loco_rs::boot::create_context::<yorishiro::App>(&env, config)
        .await
        .unwrap();
    let pool = ctx.db.get_sqlite_connection_pool();
    let mut first = pool.acquire().await.unwrap();
    let mut second = pool.acquire().await.unwrap();
    assert_ne!(first.as_mut() as *mut _, second.as_mut() as *mut _);
    for connection in [&mut first, &mut second] {
        let count: i64 = sea_orm::sqlx::query_scalar("SELECT count(*) FROM startup_probe")
            .fetch_one(&mut **connection)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let at: String = sea_orm::sqlx::query_scalar("SELECT CURRENT_TIMESTAMP")
            .fetch_one(&mut **connection)
            .await
            .unwrap();
        let parsed = chrono::DateTime::parse_from_rfc3339(&at).unwrap();
        let now = chrono::Utc::now();
        assert!((parsed.timestamp() - now.timestamp()).abs() <= 2);
    }
    drop(first);
    drop(second);
    ctx.db.close().await.unwrap();
}

#[tokio::test]
#[serial(process_environment)]
async fn sqlite_timestamp_fixture_is_independent_of_redis_queue_url() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let queue_guard = crate::EnvGuard::capture(&["QUEUE_URL"]);
    queue_guard.set("QUEUE_URL", "redis://127.0.0.1:6379/15");
    let directory = tempfile::tempdir().unwrap();
    let env = loco_rs::environment::Environment::Any("test_sqlite".into());
    let mut config = yorishiro::App::load_config(&env).await.unwrap();
    config.database.uri = format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("app.sqlite3").display()
    );
    if let Some(loco_rs::config::QueueConfig::Sqlite(queue)) = config.queue.as_mut() {
        queue.uri = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("queue.sqlite3").display()
        );
    }
    config.database.dangerously_truncate = false;
    config.database.dangerously_recreate = false;
    config.database.auto_migrate = false;
    let result = loco_rs::boot::create_context::<yorishiro::App>(&env, config).await;
    assert!(
        result.is_ok(),
        "SQLite app pool boot must not use Redis QUEUE_URL"
    );
    result.unwrap().db.close().await.unwrap();
    drop(queue_guard);
}

#[tokio::test]
async fn sqlite_timezone_state_serializes_concurrent_activations() {
    if !crate::require_sqlite_backend() {
        return;
    }
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|_request, ctx| async move {
        let db = ctx.db.clone();
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
    })
    .await;
}

#[tokio::test]
async fn sqlite_timezone_state_serializes_writers_on_separate_connections() {
    if !crate::require_sqlite_backend() {
        return;
    }
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|_request, ctx| async move {
    let db = ctx.db.clone();
    let url = ctx.config.database.uri.clone();
    yorishiro::models::timezone_state::activate(&db, "UTC")
        .await
        .unwrap();
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
        let result = yorishiro::models::timezone_state::activate(&first, "Asia/Tokyo").await;
        first.close().await.unwrap();
        result
    });
    let second_task = tokio::spawn(async move {
        let result = yorishiro::models::timezone_state::activate(&second, "America/New_York").await;
        second.close().await.unwrap();
        result
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
    same_zone.close().await.unwrap();
    audit_db.close().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn failed_activation_rolls_back_the_singleton_update() {
    if !crate::require_sqlite_backend() {
        return;
    }
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|_request, ctx| async move {
    let db = ctx.db.clone();
    yorishiro::models::timezone_state::activate(&db, "UTC")
        .await
        .unwrap();
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
    })
    .await;
}

#[tokio::test]
async fn postgres_timezone_state_serializes_concurrent_activations_without_dml_noop() {
    if !crate::require_postgres_backend() {
        return;
    }
    crate::require_sqlite_backend();
    with_timezone_scratch_db(|db, database_url| {
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

async fn sqlite_connection(url: &str) -> sea_orm::DatabaseConnection {
    use sea_orm::Database;

    let mut options = sea_orm::ConnectOptions::new(url.to_owned());
    options.max_connections(2);
    options.map_sqlx_sqlite_opts(|opts| opts.busy_timeout(Duration::from_secs(5)));
    Database::connect(options).await.unwrap()
}

static SCRATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A throwaway database, dropped and recreated so each run starts from nothing.
///
/// A `DATABASE_URL` that is present but unusable panics rather than skipping.
pub(crate) async fn scratch_db(name: &str) -> (sea_orm::DatabaseConnection, String, String) {
    let base = std::env::var("DATABASE_URL").expect("PostgreSQL DATABASE_URL");

    // Split the path from the query, so `?sslmode=require` and friends survive onto both derived
    // URLs. Dropping them silently produced a connection failure that the old `.ok()?` turned into
    // a skip, so an SSL-requiring server reported two passing tests that had not run.
    let (without_query, query) = match base.split_once('?') {
        Some((head, q)) => (head, format!("?{q}")),
        None => (base.as_str(), String::new()),
    };
    // Rewrite only the last path segment, so a password or host containing the database's name is left alone.
    let prefix = without_query
        .rsplit_once('/')
        .expect("DATABASE_URL has no database path segment")
        .0;
    let admin_url = format!("{prefix}/postgres{query}");
    let unique_name = format!(
        "yorishiro_tz_{}_{}_{}",
        std::process::id(),
        SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        name
    );
    let target_url = format!("{prefix}/{unique_name}{query}");

    let admin = Database::connect(&admin_url)
        .await
        .expect("connect to the admin database named by DATABASE_URL");
    for sql in [
        format!("DROP DATABASE IF EXISTS {unique_name}"),
        format!("CREATE DATABASE {unique_name}"),
    ] {
        admin
            .execute_unprepared(&sql)
            .await
            .expect("prepare scratch database");
    }
    drop(admin);

    let db = Database::connect(&target_url)
        .await
        .expect("connect to scratch database");
    (db, unique_name, target_url)
}

pub(crate) async fn close_scratch_db(
    db: sea_orm::DatabaseConnection,
    name: &str,
) -> Result<(), String> {
    let mut errors = Vec::new();
    if let Err(error) = db.close().await {
        errors.push(format!("close scratch database pool: {error}"));
    }
    let base = std::env::var("DATABASE_URL")
        .map_err(|error| format!("read DATABASE_URL for scratch cleanup: {error}"))?;
    let (without_query, query) = match base.split_once('?') {
        Some((head, q)) => (head, format!("?{q}")),
        None => (base.as_str(), String::new()),
    };
    let prefix = without_query
        .rsplit_once('/')
        .ok_or_else(|| "DATABASE_URL has no database path segment".to_owned())?
        .0;
    let admin = Database::connect(format!("{prefix}/postgres{query}"))
        .await
        .map_err(|error| format!("connect to the admin database for scratch cleanup: {error}"))?;
    if let Err(error) = admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS {name}"))
        .await
    {
        errors.push(format!("drop scratch database {name}: {error}"));
    }
    if let Err(error) = admin.close().await {
        errors.push(format!("close scratch admin pool: {error}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

async fn with_timezone_scratch_db<F>(test: F)
where
    F: for<'a> FnOnce(
            &'a sea_orm::DatabaseConnection,
            &'a str,
        ) -> futures::future::BoxFuture<'a, ()>
        + Send,
{
    let (db, database_name, target_url) = scratch_db("timezone_state").await;
    let result = AssertUnwindSafe(test(&db, &target_url))
        .catch_unwind()
        .await;
    let cleanup = close_scratch_db(db, &database_name).await;
    match (result, cleanup) {
        (Ok(()), Ok(())) => {}
        (Ok(()), Err(error)) => panic!("scratch database cleanup failed: {error}"),
        (Err(panic), Ok(())) => std::panic::resume_unwind(panic),
        (Err(panic), Err(error)) => {
            eprintln!("scratch database cleanup also failed: {error}");
            std::panic::resume_unwind(panic);
        }
    }
}
