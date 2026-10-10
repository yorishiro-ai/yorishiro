use loco_rs::config::QueueConfig;
use loco_rs::environment::Environment;
use serial_test::serial;

use yorishiro::data::config::load;

use super::EnvGuard;

#[tokio::test]
#[serial(process_environment)]
async fn development_uses_loco_environment_config_and_typed_settings() {
    let _guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
        "YORISHIRO_EMBEDDING_PROVIDER",
    ]);
    _guard.set("DATABASE_URL", "sqlite:///tmp/config-load.sqlite3?mode=rwc");
    _guard.set(
        "QUEUE_URL",
        "sqlite:///tmp/config-load-queue.sqlite3?mode=rwc",
    );
    _guard.remove("YORISHIRO_QUEUE_KIND");
    _guard.set("YORISHIRO_EMBEDDING_PROVIDER", "none");

    let config = load(&Environment::Development).await.unwrap();
    let settings = config.settings::<serde_json::Value>().unwrap();

    assert_eq!(
        config.database.uri,
        "sqlite:///tmp/config-load.sqlite3?mode=rwc"
    );
    assert_eq!(settings["embedding"]["provider"], "none");
}

#[tokio::test]
#[serial(process_environment)]
async fn test_environment_selects_backend_specific_loco_config() {
    let _guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_TEST_TOPOLOGY"]);
    _guard.set("YORISHIRO_TEST_TOPOLOGY", "sqlite-valkey");
    _guard.set("DATABASE_URL", "sqlite:///tmp/test.sqlite3?mode=rwc");
    _guard.set("QUEUE_URL", "redis://localhost:6379");

    let config = load(&Environment::Test).await.unwrap();

    assert!(config.database.uri.starts_with("sqlite://"));
    assert!(matches!(
        config.queue,
        Some(QueueConfig::Redis(queue)) if queue.uri == "redis://localhost:6379"
    ));
}

#[tokio::test]
#[serial(process_environment)]
async fn sqlite_database_queue_scheme_controls_test_queue_selection() {
    let guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_TEST_TOPOLOGY"]);
    guard.set("DATABASE_URL", "sqlite:///tmp/test.sqlite3?mode=rwc");

    guard.set("QUEUE_URL", "sqlite:///tmp/test-queue.sqlite3?mode=rwc");
    guard.set("YORISHIRO_TEST_TOPOLOGY", "sqlite-sqlite");
    let sqlite_queue = load(&Environment::Test).await.unwrap();
    assert!(matches!(sqlite_queue.queue, Some(QueueConfig::Sqlite(_))));

    guard.set("QUEUE_URL", "redis://localhost:6379");
    guard.set("YORISHIRO_TEST_TOPOLOGY", "sqlite-valkey");
    let redis_queue = load(&Environment::Test).await.unwrap();
    assert!(matches!(redis_queue.queue, Some(QueueConfig::Redis(_))));

    guard.set("QUEUE_URL", "rediss://localhost:6379");
    guard.set("YORISHIRO_TEST_TOPOLOGY", "sqlite-valkey");
    let rediss_queue = load(&Environment::Test).await.unwrap();
    assert!(matches!(rediss_queue.queue, Some(QueueConfig::Redis(_))));
}

#[tokio::test]
#[serial(process_environment)]
async fn redis_test_config_keeps_queue_independent_from_database() {
    let _guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_TEST_TOPOLOGY"]);
    _guard.remove("YORISHIRO_TEST_TOPOLOGY");
    _guard.set("DATABASE_URL", "postgres://test:test@localhost:5432/test");
    _guard.set("QUEUE_URL", "redis://queue.example:6379");

    let config = load(&Environment::Any("test_valkey".into())).await.unwrap();

    assert_eq!(
        config.database.uri,
        "postgres://test:test@localhost:5432/test"
    );
    assert!(matches!(
        config.queue,
        Some(QueueConfig::Redis(queue)) if queue.uri == "redis://queue.example:6379"
    ));
}

#[tokio::test]
#[serial(process_environment)]
async fn test_configs_disable_external_embedding_downloads_by_default() {
    let _guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_EMBEDDING_PROVIDER"]);
    _guard.set("DATABASE_URL", "sqlite:///tmp/test.sqlite3?mode=rwc");
    _guard.set("QUEUE_URL", "sqlite:///tmp/test-queue.sqlite3?mode=rwc");
    _guard.remove("YORISHIRO_EMBEDDING_PROVIDER");

    let config = load(&Environment::Any("test_sqlite".into())).await.unwrap();
    let settings = config.settings::<serde_json::Value>().unwrap();

    assert_eq!(settings["embedding"]["provider"], "none");
}

#[tokio::test]
#[serial(process_environment)]
async fn every_supported_topology_loads_from_loco_test_environment() {
    let guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_TEST_TOPOLOGY"]);
    for (topology, database, queue) in [
        (
            "postgres-postgres",
            "postgres://db/app",
            "postgres://queue/app",
        ),
        (
            "sqlite-sqlite",
            "sqlite://db.sqlite3?mode=rwc",
            "sqlite://queue.sqlite3?mode=rwc",
        ),
        (
            "postgres-valkey",
            "postgres://db/app",
            "redis://valkey:6379/15",
        ),
        (
            "sqlite-valkey",
            "sqlite://db.sqlite3?mode=rwc",
            "redis://valkey:6379/15",
        ),
        (
            "postgres-sqlite",
            "postgres://db/app",
            "sqlite://queue.sqlite3?mode=rwc",
        ),
        (
            "sqlite-postgres",
            "sqlite://db.sqlite3?mode=rwc",
            "postgres://queue/app",
        ),
    ] {
        guard.set("YORISHIRO_TEST_TOPOLOGY", topology);
        guard.set("DATABASE_URL", database);
        guard.set("QUEUE_URL", queue);
        let config = load(&Environment::Test).await.unwrap();
        assert_eq!(config.database.uri, database);
        match (topology.split('-').nth(1).unwrap(), config.queue.unwrap()) {
            ("postgres", QueueConfig::Postgres(actual)) => assert_eq!(actual.uri, queue),
            ("sqlite", QueueConfig::Sqlite(actual)) => assert_eq!(actual.uri, queue),
            ("valkey", QueueConfig::Redis(actual)) => assert_eq!(actual.uri, queue),
            (provider, actual) => panic!("{topology} resolved to {provider}: {actual:?}"),
        }
    }
}

#[tokio::test]
#[serial(process_environment)]
async fn production_config_is_the_packaged_loco_config() {
    let _guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_EMBEDDING_PROVIDER"]);
    _guard.remove("DATABASE_URL");
    _guard.remove("QUEUE_URL");
    _guard.remove("YORISHIRO_EMBEDDING_PROVIDER");

    let config = load(&Environment::Production).await.unwrap();
    let settings = config.settings::<serde_json::Value>().unwrap();

    assert_eq!(
        config.database.uri,
        "sqlite:///var/lib/yorishiro/yorishiro.sqlite3?mode=rwc"
    );
    assert_eq!(settings["embedding"]["provider"], "local");
}

#[tokio::test]
#[serial(process_environment)]
async fn production_selects_queue_provider_independently_from_database() {
    let _guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
        "YORISHIRO_EMBEDDING_PROVIDER",
    ]);
    _guard.set("DATABASE_URL", "postgres://db/yorishiro");
    _guard.set("YORISHIRO_EMBEDDING_PROVIDER", "none");
    _guard.remove("YORISHIRO_QUEUE_KIND");
    _guard.remove("QUEUE_URL");

    let postgres_queue = load(&Environment::Production).await.unwrap();

    assert_eq!(postgres_queue.database.uri, "postgres://db/yorishiro");
    assert!(matches!(
        postgres_queue.queue,
        Some(QueueConfig::Postgres(queue)) if queue.uri == "postgres://db/yorishiro"
    ));

    _guard.set("YORISHIRO_QUEUE_KIND", "Redis");
    _guard.set("QUEUE_URL", "redis://queue.example:6379");

    let redis_queue = load(&Environment::Production).await.unwrap();

    assert_eq!(redis_queue.database.uri, "postgres://db/yorishiro");
    assert!(matches!(
        redis_queue.queue,
        Some(QueueConfig::Redis(queue)) if queue.uri == "redis://queue.example:6379"
    ));
}

/// Production defaults follow the database and queue pair: a SQLite writer or queue gains nothing from a wide pool or several workers, while PostgreSQL and Redis-compatible queues do.
#[tokio::test]
#[serial(process_environment)]
async fn production_defaults_follow_the_database_and_queue_topology() {
    let guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
        "YORISHIRO_QUEUE_WORKERS",
        "DB_MAX_CONNECTIONS",
        "YORISHIRO_EMBEDDING_PROVIDER",
    ]);
    guard.set("YORISHIRO_EMBEDDING_PROVIDER", "none");
    guard.remove("YORISHIRO_QUEUE_WORKERS");
    guard.remove("DB_MAX_CONNECTIONS");
    for (database, kind, queue, pool, workers) in [
        ("sqlite:///var/lib/y/a.sqlite3?mode=rwc", None, None, 10, 1),
        (
            "sqlite:///var/lib/y/a.sqlite3?mode=rwc",
            Some("Redis"),
            Some("redis://valkey:6379/0"),
            10,
            2,
        ),
        ("postgres://db/app", None, None, 100, 2),
        (
            "postgres://db/app",
            Some("Redis"),
            Some("redis://valkey:6379/0"),
            100,
            2,
        ),
        (
            "postgres://db/app",
            Some("Sqlite"),
            Some("sqlite:///var/lib/y/q.sqlite3?mode=rwc"),
            100,
            1,
        ),
    ] {
        guard.set("DATABASE_URL", database);
        match kind {
            Some(kind) => guard.set("YORISHIRO_QUEUE_KIND", kind),
            None => guard.remove("YORISHIRO_QUEUE_KIND"),
        }
        match queue {
            Some(queue) => guard.set("QUEUE_URL", queue),
            None => guard.remove("QUEUE_URL"),
        }
        let config = load(&Environment::Production).await.unwrap();
        assert_eq!(config.database.max_connections, pool, "{database} pool");
        let actual = match config.queue.unwrap() {
            QueueConfig::Sqlite(queue) => queue.num_workers,
            QueueConfig::Redis(queue) => queue.num_workers,
            QueueConfig::Postgres(queue) => queue.num_workers,
            other => panic!("unexpected queue {other:?}"),
        };
        assert_eq!(actual, workers, "{database} {kind:?} workers");
    }
}

/// Each bad value is reported by name, so an operator is not left guessing which setting a failed boot objected to.
#[tokio::test]
#[serial(process_environment)]
async fn query_embedding_settings_are_validated() {
    let guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUERY_EMBEDDING_TIMEOUT_MS",
        "YORISHIRO_QUERY_EMBEDDING_POLL_INTERVAL_MS",
        "YORISHIRO_QUERY_EMBEDDING_RETENTION_SECS",
    ]);
    guard.set("DATABASE_URL", "sqlite:///tmp/qe-config.sqlite3?mode=rwc");
    guard.set(
        "QUEUE_URL",
        "sqlite:///tmp/qe-config-queue.sqlite3?mode=rwc",
    );
    let environment = Environment::Any("test_sqlite".into());

    let defaults = load(&environment).await.unwrap();
    let settings = defaults
        .settings::<yorishiro::data::settings::Settings>()
        .unwrap();
    assert_eq!(settings.query_embedding.timeout_ms, 15_000);

    for (timeout, poll, retention, expected) in [
        ("0", "100", "300", "greater than zero"),
        ("1000", "0", "300", "greater than zero"),
        ("200000", "100", "300", "must not exceed 120000"),
        ("1000", "2000", "300", "poll_interval_ms must not exceed"),
        ("15000", "100", "5", "retention_seconds must cover"),
    ] {
        guard.set("YORISHIRO_QUERY_EMBEDDING_TIMEOUT_MS", timeout);
        guard.set("YORISHIRO_QUERY_EMBEDDING_POLL_INTERVAL_MS", poll);
        guard.set("YORISHIRO_QUERY_EMBEDDING_RETENTION_SECS", retention);
        let error = load(&environment).await.unwrap_err().to_string();
        assert!(
            error.contains(expected),
            "{timeout}/{poll}/{retention}: {error}"
        );
    }
}
