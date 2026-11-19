use loco_rs::{app::Hooks, boot::StartMode, environment::Environment};

use super::minimal_config;

#[tokio::test]
async fn boot_rejects_a_shared_sqlite_database_and_queue_file() {
    let config = serde_yaml::from_str::<loco_rs::config::Config>(
        &minimal_config("sqlite://app.sqlite3?mode=rwc").replace(
            "sqlite://queue.sqlite3?mode=rwc",
            "sqlite://./app.sqlite3?mode=rw",
        ),
    )
    .unwrap();

    let error = match yorishiro::App::boot(StartMode::ServerOnly, &Environment::Test, config).await
    {
        Ok(_) => panic!("shared SQLite file should be rejected at App::boot"),
        Err(error) => error.to_string(),
    };

    assert!(error.contains("same file"), "{error}");
}
