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
async fn sqlite_test_config_preserves_loco_application_pragmas() {
    let guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_QUEUE_KIND"]);
    guard.set("DATABASE_URL", "sqlite:///tmp/config-load.sqlite3?mode=rwc");
    guard.set(
        "QUEUE_URL",
        "sqlite:///tmp/config-load-queue.sqlite3?mode=rwc",
    );
    guard.remove("YORISHIRO_QUEUE_KIND");
    let config = load(&Environment::Any("test_sqlite".into())).await.unwrap();
    assert_eq!(
        config.database.run_on_start.as_deref(),
        Some(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA mmap_size = 134217728; PRAGMA journal_size_limit = 67108864; PRAGMA cache_size = 2000; PRAGMA busy_timeout = 5000;"
        )
    );
}

#[tokio::test]
#[serial(process_environment)]
async fn the_topology_picks_the_queue_provider_for_one_database() {
    let guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_TEST_TOPOLOGY"]);
    guard.set("DATABASE_URL", "sqlite:///tmp/test.sqlite3?mode=rwc");

    guard.set("QUEUE_URL", "sqlite:///tmp/test-queue.sqlite3?mode=rwc");
    guard.set("YORISHIRO_TEST_TOPOLOGY", "sqlite-sqlite");
    let sqlite_queue = load(&Environment::Test).await.unwrap();
    assert!(matches!(sqlite_queue.queue, Some(QueueConfig::Sqlite(_))));

    guard.set("QUEUE_URL", "redis://localhost:6379");
    guard.set("YORISHIRO_TEST_TOPOLOGY", "sqlite-redis");
    let redis_queue = load(&Environment::Test).await.unwrap();
    assert!(matches!(redis_queue.queue, Some(QueueConfig::Redis(_))));

    guard.set("QUEUE_URL", "rediss://localhost:6379");
    guard.set("YORISHIRO_TEST_TOPOLOGY", "sqlite-redis");
    let rediss_queue = load(&Environment::Test).await.unwrap();
    assert!(matches!(rediss_queue.queue, Some(QueueConfig::Redis(_))));
}

/// Without a topology, the test environment follows `DATABASE_URL`'s scheme and the queue follows the database.
#[tokio::test]
#[serial(process_environment)]
async fn the_test_environment_follows_the_database_scheme_without_a_topology() {
    let guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_TEST_TOPOLOGY",
        "DB_CONNECT_TIMEOUT",
    ]);
    guard.remove("YORISHIRO_TEST_TOPOLOGY");
    guard.remove("QUEUE_URL");

    guard.set("DATABASE_URL", "sqlite:///tmp/scheme.sqlite3?mode=rwc");
    let sqlite = load(&Environment::Test).await.unwrap();
    assert!(matches!(sqlite.queue, Some(QueueConfig::Sqlite(_))));

    guard.set("DATABASE_URL", "postgres://db/app");
    guard.set("DB_CONNECT_TIMEOUT", "5000");
    let postgres = load(&Environment::Test).await.unwrap();
    assert!(matches!(
        postgres.queue,
        Some(QueueConfig::Postgres(ref queue)) if queue.uri == "postgres://db/app"
            && queue.connect_timeout == postgres.database.connect_timeout
            && queue.connect_timeout == 5000
    ));
    assert_eq!(postgres.database.uri, "postgres://db/app");
    let topology = load(&Environment::Any("test_postgres".into()))
        .await
        .unwrap();
    assert!(
        matches!(topology.queue, Some(QueueConfig::Postgres(queue)) if queue.connect_timeout == 5000)
    );
}

#[tokio::test]
#[serial(process_environment)]
async fn redis_test_config_keeps_queue_independent_from_database() {
    let _guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_TEST_TOPOLOGY"]);
    _guard.remove("YORISHIRO_TEST_TOPOLOGY");
    _guard.set("DATABASE_URL", "postgres://test:test@localhost:5432/test");
    _guard.set("QUEUE_URL", "redis://queue.example.com:6379");

    let config = load(&Environment::Any("test_sqlite_redis".into()))
        .await
        .unwrap();

    assert_eq!(
        config.database.uri,
        "postgres://test:test@localhost:5432/test"
    );
    assert!(matches!(
        config.queue,
        Some(QueueConfig::Redis(queue)) if queue.uri == "redis://queue.example.com:6379"
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
    let guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_TEST_TOPOLOGY",
        "PORT",
    ]);
    guard.set("PORT", "5151");
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
            "postgres-redis",
            "postgres://db/app",
            "redis://redis:6379/15",
        ),
        (
            "sqlite-redis",
            "sqlite://db.sqlite3?mode=rwc",
            "redis://redis:6379/15",
        ),
    ] {
        guard.set("YORISHIRO_TEST_TOPOLOGY", topology);
        guard.set("DATABASE_URL", database);
        guard.set("QUEUE_URL", queue);
        let config = load(&Environment::Test).await.unwrap();
        assert_eq!(config.database.uri, database);
        assert_eq!(config.server.port, 5151, "{topology} must honor PORT");
        match (topology.split('-').nth(1).unwrap(), config.queue.unwrap()) {
            ("postgres", QueueConfig::Postgres(actual)) => {
                assert_eq!(actual.uri, queue);
                assert_eq!(actual.connect_timeout, config.database.connect_timeout);
                assert_eq!(actual.max_connections, config.database.max_connections);
                assert_eq!(actual.min_connections, config.database.min_connections);
                assert_eq!(actual.idle_timeout, config.database.idle_timeout);
            }
            ("sqlite", QueueConfig::Sqlite(actual)) => {
                assert_eq!(actual.uri, queue);
                assert_eq!(actual.connect_timeout, config.database.connect_timeout);
                assert_eq!(actual.max_connections, config.database.max_connections);
                assert_eq!(actual.min_connections, config.database.min_connections);
                assert_eq!(actual.idle_timeout, config.database.idle_timeout);
            }
            ("redis", QueueConfig::Redis(actual)) => {
                assert_eq!(actual.uri, queue);
                assert_eq!(actual.connect_timeout, 10_000);
                assert_eq!(actual.response_timeout, 5_000);
            }
            (provider, actual) => panic!("{topology} resolved to {provider}: {actual:?}"),
        }
    }
}

#[tokio::test]
#[serial(process_environment)]
async fn production_config_is_the_packaged_loco_config() {
    let _guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
        "YORISHIRO_EMBEDDING_PROVIDER",
    ]);
    _guard.remove("DATABASE_URL");
    _guard.remove("QUEUE_URL");
    _guard.remove("YORISHIRO_QUEUE_KIND");
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
async fn production_example_is_used_when_local_config_is_absent() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/config/production.yaml"),
        directory.path().join("production.yaml.example"),
    )
    .unwrap();
    let guard = EnvGuard::capture(&[
        "LOCO_CONFIG_FOLDER",
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_EMBEDDING_PROVIDER",
    ]);
    guard.set("LOCO_CONFIG_FOLDER", directory.path().to_str().unwrap());
    guard.set(
        "DATABASE_URL",
        "sqlite:///tmp/packaged-fallback.sqlite3?mode=rwc",
    );
    guard.set(
        "QUEUE_URL",
        "sqlite:///tmp/packaged-fallback-queue.sqlite3?mode=rwc",
    );
    guard.set("YORISHIRO_EMBEDDING_PROVIDER", "none");

    let config = load(&Environment::Production).await.unwrap();
    assert_eq!(
        config.database.uri,
        "sqlite:///tmp/packaged-fallback.sqlite3?mode=rwc"
    );
    assert!(!directory.path().join("production.yaml").exists());
    std::fs::rename(
        directory.path().join("production.yaml.example"),
        directory.path().join("production.yaml"),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("production.local.yaml"),
        "queue:\n  connect_timeout: 1234\n  max_connections: 3\n",
    )
    .unwrap();
    let overridden =
        loco_rs::config::Config::from_folder(&Environment::Production, directory.path())
            .expect("framework resolves SQL defaults after local overrides");
    let Some(QueueConfig::Sqlite(queue)) = overridden.queue else {
        panic!("the local override must preserve the selected SQL provider");
    };
    assert_eq!(queue.connect_timeout, 1234);
    assert_eq!(queue.max_connections, 3);
    assert_eq!(queue.idle_timeout, overridden.database.idle_timeout);
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
    _guard.set("QUEUE_URL", "redis://queue.example.com:6379");

    let redis_queue = load(&Environment::Production).await.unwrap();

    assert_eq!(redis_queue.database.uri, "postgres://db/yorishiro");
    assert!(matches!(
        redis_queue.queue,
        Some(QueueConfig::Redis(queue)) if queue.uri == "redis://queue.example.com:6379"
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
        ("sqlite:/var/lib/y/a.sqlite3?mode=rwc", None, None, 10, 1),
        ("sqlite::memory:", None, None, 10, 1),
        (
            "sqlite:///var/lib/y/a.sqlite3?mode=rwc",
            Some("Redis"),
            Some("redis://redis:6379/0"),
            10,
            2,
        ),
        ("postgres://db/app", None, None, 100, 2),
        (
            "postgres://db/app",
            Some("Redis"),
            Some("redis://redis:6379/0"),
            100,
            2,
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

/// Invalid SQL pairs fail during config loading, before any database or queue connection is opened.
#[tokio::test]
#[serial(process_environment)]
async fn production_refuses_crossed_sql_queue_backends() {
    let guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
        "YORISHIRO_EMBEDDING_PROVIDER",
    ]);
    guard.set("YORISHIRO_EMBEDDING_PROVIDER", "none");
    for (database, kind, queue) in [
        (
            "postgres://db/app",
            "Sqlite",
            "sqlite:///var/lib/y/q.sqlite3?mode=rwc",
        ),
        (
            "sqlite:///var/lib/y/a.sqlite3?mode=rwc",
            "Postgres",
            "postgres://queue/app",
        ),
    ] {
        guard.set("DATABASE_URL", database);
        guard.set("YORISHIRO_QUEUE_KIND", kind);
        guard.set("QUEUE_URL", queue);
        let loco_rs::Error::CustomError(_, detail) = load(&Environment::Production)
            .await
            .expect_err("invalid pair must not boot")
        else {
            panic!("expected ValidationFailed");
        };
        assert_eq!(detail.errors.unwrap()["error"]["code"], "validation_failed");
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

#[tokio::test]
#[serial(process_environment)]
async fn mcp_allowed_hosts_load_and_reject_wildcards() {
    let guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_MCP_ALLOWED_HOSTS"]);
    guard.set("DATABASE_URL", "sqlite:///tmp/mcp-config.sqlite3?mode=rwc");
    guard.set(
        "QUEUE_URL",
        "sqlite:///tmp/mcp-config-queue.sqlite3?mode=rwc",
    );
    guard.set(
        "YORISHIRO_MCP_ALLOWED_HOSTS",
        "mcp.example.com,10.0.0.4:5150",
    );
    let config = load(&Environment::Any("test_sqlite".into())).await.unwrap();
    let settings = config.settings::<serde_json::Value>().unwrap();
    assert_eq!(
        settings["mcp"]["allowed_hosts"],
        "mcp.example.com,10.0.0.4:5150"
    );

    guard.set("YORISHIRO_MCP_ALLOWED_HOSTS", "*");
    let error = load(&Environment::Any("test_sqlite".into()))
        .await
        .expect_err("wildcard MCP hosts must be rejected");
    assert!(
        error.to_string().contains("wildcard hosts")
            || error.to_string().contains("wildcard MCP hosts"),
        "unexpected validation error: {error}"
    );
}

#[tokio::test]
#[serial(process_environment)]
async fn mcp_network_policy_follows_binding_and_validates_origins() {
    use axum::http::StatusCode;
    use rmcp::transport::streamable_http_server::{
        StreamableHttpService, session::local::LocalSessionManager,
    };
    use yorishiro::controllers::mcp::transport_config;

    struct EmptyServer;
    impl rmcp::ServerHandler for EmptyServer {}

    let guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "BINDING",
        "YORISHIRO_MCP_ALLOWED_HOSTS",
        "YORISHIRO_ALLOWED_ORIGINS",
    ]);
    guard.set("DATABASE_URL", "sqlite:///tmp/mcp-policy.sqlite3?mode=rwc");
    guard.set(
        "QUEUE_URL",
        "sqlite:///tmp/mcp-policy-queue.sqlite3?mode=rwc",
    );
    guard.remove("BINDING");
    guard.remove("YORISHIRO_MCP_ALLOWED_HOSTS");
    guard.remove("YORISHIRO_ALLOWED_ORIGINS");
    let mut config = load(&Environment::Any("test_sqlite".into())).await.unwrap();
    assert_eq!(config.server.binding, "localhost");
    let production = load(&Environment::Production).await.unwrap();
    assert_eq!(production.server.binding, "localhost");

    for (binding, host, origin, expected) in [
        (
            "localhost",
            "localhost:5150",
            None,
            StatusCode::NOT_ACCEPTABLE,
        ),
        (
            "127.0.0.1",
            "127.0.0.1:5150",
            None,
            StatusCode::NOT_ACCEPTABLE,
        ),
        ("::1", "[::1]:5150", None, StatusCode::NOT_ACCEPTABLE),
        (
            "localhost",
            "untrusted.example.com",
            None,
            StatusCode::FORBIDDEN,
        ),
        (
            "192.0.2.10",
            "192.0.2.10:5150",
            None,
            StatusCode::NOT_ACCEPTABLE,
        ),
        ("192.0.2.10", "192.0.2.11:5150", None, StatusCode::FORBIDDEN),
        (
            "0.0.0.0",
            "example.com:5150",
            None,
            StatusCode::NOT_ACCEPTABLE,
        ),
        ("::", "example.com:5150", None, StatusCode::NOT_ACCEPTABLE),
        (
            "0.0.0.0",
            "example.com",
            Some("https://evil.example.com"),
            StatusCode::FORBIDDEN,
        ),
        (
            "localhost",
            "localhost",
            Some("null"),
            StatusCode::FORBIDDEN,
        ),
    ] {
        config.server.binding = binding.into();
        let service = StreamableHttpService::new(
            || Ok(EmptyServer),
            LocalSessionManager::default().into(),
            transport_config(&config).unwrap(),
        );
        let server =
            loco_rs::TestServer::new(axum::Router::new().fallback_service(service)).unwrap();
        let mut request = server.post("/").add_header("Host", host);
        if let Some(origin) = origin {
            request = request.add_header("Origin", origin);
        }
        let response = request.await;
        assert_eq!(
            response.status_code(),
            expected,
            "{binding} / {host} / {origin:?}"
        );
    }

    guard.set("YORISHIRO_ALLOWED_ORIGINS", r#"["https://example.com"]"#);
    config = load(&Environment::Any("test_sqlite".into())).await.unwrap();
    let service = StreamableHttpService::new(
        || Ok(EmptyServer),
        LocalSessionManager::default().into(),
        transport_config(&config).unwrap(),
    );
    let cors = config
        .server
        .middlewares
        .cors
        .as_ref()
        .unwrap()
        .cors()
        .unwrap();
    let server = loco_rs::TestServer::new(
        axum::Router::new()
            .route("/rest", axum::routing::post(|| async { "ok" }))
            .nest_service("/mcp", service)
            .layer(cors),
    )
    .unwrap();
    for path in ["/rest", "/mcp"] {
        let response = server
            .method(axum::http::Method::OPTIONS, path)
            .add_header("Host", "localhost")
            .add_header("Origin", "https://example.com")
            .add_header("Access-Control-Request-Method", "POST")
            .add_header(
                "Access-Control-Request-Headers",
                "authorization,mcp-session-id",
            )
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
        assert_eq!(
            response.header("access-control-allow-origin"),
            "https://example.com"
        );
        let denied = server
            .method(axum::http::Method::OPTIONS, path)
            .add_header("Origin", "https://evil.example.com")
            .add_header("Access-Control-Request-Method", "POST")
            .await;
        assert!(!denied.headers().contains_key("access-control-allow-origin"));
    }
    let response = server
        .post("/mcp")
        .add_header("Host", "localhost")
        .add_header("Origin", "https://example.com")
        .await;
    assert_eq!(response.status_code(), StatusCode::NOT_ACCEPTABLE);
    let wrong_port = server
        .post("/mcp")
        .add_header("Host", "localhost")
        .add_header("Origin", "https://example.com:8443")
        .await;
    assert_eq!(wrong_port.status_code(), StatusCode::FORBIDDEN);
    for origin in [
        "*",
        "example.com",
        "https://example.com:443/path",
        "null",
        "https://user@example.com",
        "https://*.example.com",
        "https://example.com?x=1",
        "https://example.com/",
    ] {
        guard.set(
            "YORISHIRO_ALLOWED_ORIGINS",
            serde_json::to_string(&[origin]).unwrap(),
        );
        assert!(
            load(&Environment::Any("test_sqlite".into())).await.is_err(),
            "{origin}"
        );
    }
}
