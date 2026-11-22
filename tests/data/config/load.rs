use loco_rs::config::QueueConfig;
use loco_rs::environment::Environment;
use serial_test::serial;

use yorishiro::data::config::load;

use super::EnvGuard;

#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
async fn deployment_worker_targets_default_to_shared_and_require_valid_tags() {
    let guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_WORKER_TARGETS"]);
    guard.set(
        "DATABASE_URL",
        "sqlite:///tmp/worker-target-config.sqlite3?mode=rwc",
    );
    guard.set(
        "QUEUE_URL",
        "sqlite:///tmp/worker-target-queue.sqlite3?mode=rwc",
    );
    guard.remove("YORISHIRO_WORKER_TARGETS");
    let config = load(&Environment::Development).unwrap();
    let settings = config
        .settings::<yorishiro::data::settings::Settings>()
        .unwrap();
    assert_eq!(settings.worker_targets, vec!["worker-target:shared"]);
    guard.set(
        "YORISHIRO_WORKER_TARGETS",
        r#"["worker-target:shared","worker-target:official"]"#,
    );
    assert!(load(&Environment::Development).is_ok());
    for value in [
        "[]",
        r#"["official"]"#,
        r#"["worker-target:tenant:not-a-uuid"]"#,
    ] {
        guard.set("YORISHIRO_WORKER_TARGETS", value);
        let error = load(&Environment::Development).expect_err("invalid target declaration");
        assert!(
            error.to_string().contains("settings.worker_targets"),
            "{error}"
        );
    }
}

#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
async fn development_uses_loco_environment_config_and_typed_settings() {
    let guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
        "YORISHIRO_EMBEDDING_PROVIDER",
    ]);
    guard.set("DATABASE_URL", "sqlite:///tmp/config-load.sqlite3?mode=rwc");
    guard.set(
        "QUEUE_URL",
        "sqlite:///tmp/config-load-queue.sqlite3?mode=rwc",
    );
    guard.remove("YORISHIRO_QUEUE_KIND");
    guard.set("YORISHIRO_EMBEDDING_PROVIDER", "none");

    let config = load(&Environment::Development).unwrap();
    let settings = config.settings::<serde_json::Value>().unwrap();

    assert_eq!(
        config.database.uri,
        "sqlite:///tmp/config-load.sqlite3?mode=rwc"
    );
    assert_eq!(settings["embedding"]["provider"], "none");
}

#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
async fn sqlite_test_config_preserves_loco_application_pragmas() {
    let guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_QUEUE_KIND"]);
    guard.set("DATABASE_URL", "sqlite:///tmp/config-load.sqlite3?mode=rwc");
    guard.set(
        "QUEUE_URL",
        "sqlite:///tmp/config-load-queue.sqlite3?mode=rwc",
    );
    guard.remove("YORISHIRO_QUEUE_KIND");
    let config = load(&Environment::Test).unwrap();
    assert_eq!(
        config.database.run_on_start.as_deref(),
        Some(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA mmap_size = 134217728; PRAGMA journal_size_limit = 67108864; PRAGMA cache_size = 2000; PRAGMA busy_timeout = 5000;"
        )
    );
}

/// The test environment follows `DATABASE_URL`'s scheme, and the queue follows the database unless `YORISHIRO_QUEUE_KIND` names a provider.
#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
async fn the_test_environment_follows_the_database_and_queue_kind() {
    let guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
        "DB_CONNECT_TIMEOUT",
        "PORT",
    ]);
    guard.set("PORT", "5151");
    guard.set("DB_CONNECT_TIMEOUT", "5000");
    for (database, kind, queue) in [
        ("postgres://db/app", None, "postgres://queue/app"),
        (
            "sqlite://db.sqlite3?mode=rwc",
            None,
            "sqlite://queue.sqlite3?mode=rwc",
        ),
        ("postgres://db/app", Some("Redis"), "redis://redis:6379/15"),
        (
            "sqlite://db.sqlite3?mode=rwc",
            Some("Redis"),
            "rediss://redis:6379/15",
        ),
    ] {
        match kind {
            Some(kind) => guard.set("YORISHIRO_QUEUE_KIND", kind),
            None => guard.remove("YORISHIRO_QUEUE_KIND"),
        }
        guard.set("DATABASE_URL", database);
        guard.set("QUEUE_URL", queue);
        let config = load(&Environment::Test).unwrap();
        assert_eq!(config.database.uri, database);
        assert_eq!(config.server.port, 5151);
        match config.queue.unwrap() {
            QueueConfig::Postgres(actual) => {
                assert!(database.starts_with("postgres") && kind.is_none());
                assert_eq!(actual.uri, queue);
                assert_eq!(actual.connect_timeout, config.database.connect_timeout);
                assert_eq!(actual.max_connections, config.database.max_connections);
            }
            QueueConfig::Sqlite(actual) => {
                assert!(database.starts_with("sqlite") && kind.is_none());
                assert_eq!(actual.uri, queue);
            }
            QueueConfig::Redis(actual) => {
                assert!(kind.is_some());
                assert_eq!(actual.uri, queue);
            }
            other => panic!("unexpected queue provider: {other:?}"),
        }
    }
}

/// With no queue URL set, the queue shares the database URL.
#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
async fn a_postgres_test_queue_defaults_to_the_database() {
    let guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_QUEUE_KIND"]);
    guard.remove("YORISHIRO_QUEUE_KIND");
    guard.remove("QUEUE_URL");
    guard.set("DATABASE_URL", "postgres://db/app");
    let config = load(&Environment::Test).unwrap();
    assert!(matches!(
        config.queue,
        Some(QueueConfig::Postgres(ref queue)) if queue.uri == "postgres://db/app"
    ));
}

#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
async fn test_configs_disable_external_embedding_downloads_by_default() {
    let guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL", "YORISHIRO_EMBEDDING_PROVIDER"]);
    guard.set("DATABASE_URL", "sqlite:///tmp/test.sqlite3?mode=rwc");
    guard.set("QUEUE_URL", "sqlite:///tmp/test-queue.sqlite3?mode=rwc");
    guard.remove("YORISHIRO_EMBEDDING_PROVIDER");

    let config = load(&Environment::Test).unwrap();
    let settings = config.settings::<serde_json::Value>().unwrap();

    assert_eq!(settings["embedding"]["provider"], "none");
}

#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
async fn production_config_is_the_packaged_loco_config() {
    let guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
        "YORISHIRO_EMBEDDING_PROVIDER",
    ]);
    guard.remove("DATABASE_URL");
    guard.remove("QUEUE_URL");
    guard.remove("YORISHIRO_QUEUE_KIND");
    guard.remove("YORISHIRO_EMBEDDING_PROVIDER");

    let config = load(&Environment::Production).unwrap();
    let settings = config.settings::<serde_json::Value>().unwrap();

    assert_eq!(
        config.database.uri,
        "sqlite:///var/lib/yorishiro/yorishiro.sqlite3?mode=rwc"
    );
    assert_eq!(settings["embedding"]["provider"], "local");
}

#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
async fn production_uses_embedded_defaults_and_ignores_the_example() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("production.yaml.example"),
        "invalid: [",
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

    let config = load(&Environment::Production).unwrap();
    let file_config = loco_rs::config::Config::from_folder(
        &Environment::Production,
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/config")),
    )
    .unwrap();
    assert!(
        serde_json::to_value(&config).unwrap() == serde_json::to_value(&file_config).unwrap(),
        "embedded and file-based production settings differ"
    );
    assert_eq!(
        config.database.uri,
        "sqlite:///tmp/packaged-fallback.sqlite3?mode=rwc"
    );
    assert!(!directory.path().join("production.yaml").exists());
    std::fs::remove_file(directory.path().join("production.yaml.example")).unwrap();
    let config_without_example = load(&Environment::Production).unwrap();
    assert_eq!(config_without_example.database.uri, config.database.uri);
}

#[cfg(unix)]
#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
async fn config_folder_preserves_non_utf8_paths() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join(OsStr::from_bytes(b"config-\xff"));
    std::fs::create_dir(&folder).unwrap();
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/config/production.yaml"),
        folder.join("production.yaml"),
    )
    .unwrap();

    let guard = EnvGuard::capture(&[
        "LOCO_CONFIG_FOLDER",
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
        "YORISHIRO_EMBEDDING_PROVIDER",
    ]);
    guard.set("LOCO_CONFIG_FOLDER", &folder);
    guard.set("DATABASE_URL", "sqlite::memory:");
    guard.set(
        "QUEUE_URL",
        "sqlite:///tmp/non-utf8-config-queue.sqlite3?mode=rwc",
    );
    guard.set("YORISHIRO_QUEUE_KIND", "Sqlite");
    guard.set("YORISHIRO_EMBEDDING_PROVIDER", "none");

    let config = load(&Environment::Production).unwrap();
    assert_eq!(config.database.uri, "sqlite::memory:");
}

#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
async fn production_selects_queue_provider_independently_from_database() {
    let guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
        "YORISHIRO_EMBEDDING_PROVIDER",
    ]);
    guard.set("DATABASE_URL", "postgres://db/yorishiro");
    guard.set("YORISHIRO_EMBEDDING_PROVIDER", "none");
    guard.remove("YORISHIRO_QUEUE_KIND");
    guard.remove("QUEUE_URL");

    let postgres_queue = load(&Environment::Production).unwrap();

    assert_eq!(postgres_queue.database.uri, "postgres://db/yorishiro");
    assert!(matches!(
        postgres_queue.queue,
        Some(QueueConfig::Postgres(queue)) if queue.uri == "postgres://db/yorishiro"
    ));

    guard.set("YORISHIRO_QUEUE_KIND", "Redis");
    guard.set("QUEUE_URL", "redis://queue.example.com:6379");

    let redis_queue = load(&Environment::Production).unwrap();

    assert_eq!(redis_queue.database.uri, "postgres://db/yorishiro");
    assert!(matches!(
        redis_queue.queue,
        Some(QueueConfig::Redis(queue)) if queue.uri == "redis://queue.example.com:6379"
    ));
}

/// Production defaults follow the database and queue pair: a SQLite writer or queue gains nothing from a wide pool or several workers, while PostgreSQL and Redis-compatible queues do.
#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
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
        let config = load(&Environment::Production).unwrap();
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
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
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
        let loco_rs::Error::CustomError(_, detail) =
            load(&Environment::Production).expect_err("invalid pair must not boot")
        else {
            panic!("expected ValidationFailed");
        };
        assert_eq!(detail.errors.unwrap()["error"]["code"], "validation_failed");
    }
}

/// Each bad value is reported by name, so an operator is not left guessing which setting a failed boot objected to.
#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
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
    let environment = Environment::Test;

    let defaults = load(&environment).unwrap();
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
        let error = load(&environment).unwrap_err().to_string();
        assert!(
            error.contains(expected),
            "{timeout}/{poll}/{retention}: {error}"
        );
    }
}

#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
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
    let config = load(&Environment::Test).unwrap();
    let settings = config.settings::<serde_json::Value>().unwrap();
    assert_eq!(
        settings["mcp"]["allowed_hosts"],
        "mcp.example.com,10.0.0.4:5150"
    );

    guard.set("YORISHIRO_MCP_ALLOWED_HOSTS", "*");
    let error = load(&Environment::Test).expect_err("wildcard MCP hosts must be rejected");
    assert!(
        error.to_string().contains("wildcard hosts")
            || error.to_string().contains("wildcard MCP hosts"),
        "unexpected validation error: {error}"
    );
}

#[tokio::test]
#[serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
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
    let mut config = load(&Environment::Test).unwrap();
    assert_eq!(config.server.binding, "localhost");
    let production = load(&Environment::Production).unwrap();
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
    config = load(&Environment::Test).unwrap();
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
        assert!(load(&Environment::Test).is_err(), "{origin}");
    }
}
