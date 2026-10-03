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
    let _guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL"]);
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
async fn redis_test_config_keeps_queue_independent_from_database() {
    let _guard = EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL"]);
    _guard.set("DATABASE_URL", "postgres://test:test@localhost:5432/test");
    _guard.set("QUEUE_URL", "redis://queue.example:6379");

    let config = load(&Environment::Any("test_redis".into())).await.unwrap();

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
