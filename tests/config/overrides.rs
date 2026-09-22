use std::fs;

use loco_rs::environment::Environment;
use serial_test::serial;
use tempfile::tempdir;
use yorishiro::config::{CANONICAL_CONFIG_FILE, load};

use super::{CurrentDirGuard, EnvGuard, minimal_config};

#[tokio::test]
#[serial]
async fn postgres_database_derives_postgres_queue_and_uri() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join(CANONICAL_CONFIG_FILE),
        minimal_config("sqlite://file.sqlite3?mode=rwc"),
    )
    .unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&[
        "YORISHIRO_CONFIG_PATH",
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
    ]);
    unsafe {
        std::env::remove_var("YORISHIRO_CONFIG_PATH");
        std::env::set_var("DATABASE_URL", "postgres://db/app");
        std::env::remove_var("QUEUE_URL");
        std::env::remove_var("YORISHIRO_QUEUE_KIND");
    }
    let config = load(&Environment::Development).await.unwrap();
    assert!(matches!(
        config.queue,
        Some(loco_rs::config::QueueConfig::Postgres(queue))
            if queue.uri == "postgres://db/app"
    ));
}

#[tokio::test]
#[serial]
async fn sqlite_database_derives_sqlite_queue_and_uri() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join(CANONICAL_CONFIG_FILE),
        minimal_config("postgres://file/app"),
    )
    .unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&[
        "YORISHIRO_CONFIG_PATH",
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
    ]);
    unsafe {
        std::env::remove_var("YORISHIRO_CONFIG_PATH");
        std::env::set_var("DATABASE_URL", "sqlite://derived.sqlite3?mode=rwc");
        std::env::remove_var("QUEUE_URL");
        std::env::remove_var("YORISHIRO_QUEUE_KIND");
    }
    let config = load(&Environment::Development).await.unwrap();
    assert!(matches!(
        config.queue,
        Some(loco_rs::config::QueueConfig::Sqlite(queue))
            if queue.uri == "sqlite://derived.sqlite3?mode=rwc"
    ));
}

#[tokio::test]
#[serial]
async fn explicit_queue_kind_and_queue_url_take_precedence() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join(CANONICAL_CONFIG_FILE),
        minimal_config("sqlite://file.sqlite3?mode=rwc"),
    )
    .unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&[
        "YORISHIRO_CONFIG_PATH",
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
    ]);
    unsafe {
        std::env::remove_var("YORISHIRO_CONFIG_PATH");
        std::env::set_var("DATABASE_URL", "postgres://db/app");
        std::env::set_var("QUEUE_URL", "sqlite://queue.sqlite3?mode=rwc");
        std::env::set_var("YORISHIRO_QUEUE_KIND", "Sqlite");
    }
    let config = load(&Environment::Development).await.unwrap();
    assert!(matches!(
        config.queue,
        Some(loco_rs::config::QueueConfig::Sqlite(queue))
            if queue.uri == "sqlite://queue.sqlite3?mode=rwc"
    ));
}

#[tokio::test]
#[serial]
async fn explicit_postgres_kind_rejects_sqlite_database_without_queue_url() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join(CANONICAL_CONFIG_FILE),
        minimal_config("sqlite://file.sqlite3?mode=rwc"),
    )
    .unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&[
        "YORISHIRO_CONFIG_PATH",
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
    ]);
    unsafe {
        std::env::remove_var("YORISHIRO_CONFIG_PATH");
        std::env::set_var("DATABASE_URL", "sqlite://db.sqlite3?mode=rwc");
        std::env::remove_var("QUEUE_URL");
        std::env::set_var("YORISHIRO_QUEUE_KIND", "Postgres");
    }
    let error = load(&Environment::Development)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("no queue URI is compatible"));
}

#[tokio::test]
#[serial]
async fn explicit_postgres_kind_rejects_sqlite_queue_url() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join(CANONICAL_CONFIG_FILE),
        minimal_config("postgres://file/app"),
    )
    .unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&[
        "YORISHIRO_CONFIG_PATH",
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
    ]);
    unsafe {
        std::env::remove_var("YORISHIRO_CONFIG_PATH");
        std::env::set_var("DATABASE_URL", "postgres://db/app");
        std::env::set_var("QUEUE_URL", "sqlite://queue.sqlite3?mode=rwc");
        std::env::set_var("YORISHIRO_QUEUE_KIND", "Postgres");
    }
    let error = load(&Environment::Development)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("QUEUE_URL is not compatible"));
}

#[tokio::test]
#[serial]
async fn redis_without_queue_url_is_rejected() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join(CANONICAL_CONFIG_FILE),
        minimal_config("sqlite://file.sqlite3?mode=rwc"),
    )
    .unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&[
        "YORISHIRO_CONFIG_PATH",
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
    ]);
    unsafe {
        std::env::remove_var("YORISHIRO_CONFIG_PATH");
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("QUEUE_URL");
        std::env::set_var("YORISHIRO_QUEUE_KIND", "Redis");
    }
    let error = load(&Environment::Development)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("QUEUE_URL is required"));
}
