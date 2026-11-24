use super::minimal_config;

#[test]
fn validation_rejects_a_shared_sqlite_database_and_queue_file() {
    let config = serde_yaml::from_str::<loco_rs::config::Config>(
        &minimal_config("sqlite://app.sqlite3?mode=rwc").replace(
            "sqlite://queue.sqlite3?mode=rwc",
            "sqlite://./app.sqlite3?mode=rw",
        ),
    )
    .unwrap();

    let error = yorishiro::data::config::validate(&config).unwrap_err();

    assert!(error.to_string().contains("same file"), "{error}");
}
