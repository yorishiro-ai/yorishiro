use chrono::Offset;
use loco_rs::app::Hooks;
use sea_orm::{ConnectionTrait, Statement};
use serial_test::serial;
use std::sync::Arc;

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
        config.database.min_connections = 1;
        config.database.max_connections = 2;
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
        // Hold the other slot so the released scoped session must be reused.
        let tenant_id = uuid::Uuid::now_v7();
        let workspace_id = uuid::Uuid::now_v7();
        let mut scoped = handle
            .tenant
            .acquire_for_workspace(tenant_id, workspace_id)
            .await
            .unwrap();
        let (pid, tenant, workspace): (i32, String, String) = sea_orm::sqlx::query_as(
            "SELECT pg_backend_pid(), current_setting('app.current_tenant'), current_setting('app.current_workspace')",
        )
        .fetch_one(&mut *scoped)
        .await
        .unwrap();
        assert_eq!(tenant, tenant_id.to_string());
        assert_eq!(workspace, workspace_id.to_string());
        let other = handle.tenant.pool().acquire().await.unwrap();
        drop(scoped);
        let mut reused = handle.tenant.pool().acquire().await.unwrap();
        let (reused_pid, tenant, workspace): (i32, String, String) = sea_orm::sqlx::query_as(
            "SELECT pg_backend_pid(), current_setting('app.current_tenant'), current_setting('app.current_workspace')",
        )
        .fetch_one(&mut *reused)
        .await
        .unwrap();
        assert_eq!(reused_pid, pid);
        assert!(tenant.is_empty(), "tenant scope leaked across pool release");
        assert!(workspace.is_empty(), "workspace scope leaked across pool release");
        drop(reused);
        drop(other);
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
    let queue_guard = crate::EnvGuard::capture(&[crate::env::TestEnv::QueueUrl.name()]);
    queue_guard.set(
        crate::env::TestEnv::QueueUrl.name(),
        "redis://127.0.0.1:6379/15",
    );
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
