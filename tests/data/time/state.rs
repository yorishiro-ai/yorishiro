use loco_rs::app::Hooks;
use serial_test::serial;

#[tokio::test]
#[serial(process_environment)]
async fn sqlite_timestamp_fixture_is_independent_of_redis_queue_url() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let queue_guard = crate::EnvGuard::capture(&["QUEUE_URL"]);
    queue_guard.set("QUEUE_URL", "redis://127.0.0.1:6379/15");
    let directory = tempfile::tempdir().unwrap();
    let env = loco_rs::environment::Environment::Test;
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
