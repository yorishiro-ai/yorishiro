use migration::MigratorTrait;
use yorishiro::app::cli_requires_timezone_gate;

#[test]
fn gates_worker_commands_after_global_environment_options() {
    for args in [
        vec!["app", "task", "sync"],
        vec!["app", "-e", "production", "task", "sync"],
        vec!["app", "--environment=production", "scheduler"],
        vec!["app", "task", "-e", "production", "sync"],
        vec!["app", "--verbose", "jobs", "purge"],
        vec!["app", "-eproduction", "jobs", "purge"],
    ] {
        assert!(cli_requires_timezone_gate(args));
    }
}

#[test]
fn does_not_gate_bootstrap_or_command_like_environment_values() {
    for args in [
        vec!["app", "db", "migrate"],
        vec!["app", "db", "entities"],
        vec!["app", "generate", "model"],
        vec!["app", "-e", "task", "routes"],
        vec!["app", "--environment", "scheduler", "routes"],
        vec!["app", "db", "migrate", "--environment", "production"],
    ] {
        assert!(!cli_requires_timezone_gate(args));
    }
}

#[test]
fn task_gate_does_not_treat_flags_as_commands() {
    for args in [
        vec!["app", "--help"],
        vec!["app", "db", "migrate", "-e", "production"],
    ] {
        assert!(!cli_requires_timezone_gate(args));
    }
}

#[test]
fn persisted_state_gate_only_applies_to_work_commands() {
    assert!(yorishiro::app::cli_requires_timezone_gate([
        "app", "task", "sync"
    ]));
    assert!(!yorishiro::app::cli_requires_timezone_gate([
        "app", "db", "migrate"
    ]));
    assert!(yorishiro::app::cli_requires_timezone_gate([
        "app",
        "scheduler"
    ]));
}

#[tokio::test]
async fn migration_bootstrap_seeds_state_for_the_task_gate() {
    use sea_orm::{Database, EntityTrait};

    let migration_args = ["app", "db", "migrate"];
    assert!(!yorishiro::app::cli_requires_timezone_gate(migration_args));
    let dir = tempfile::tempdir().unwrap();
    let db = Database::connect(format!(
        "sqlite://{}?mode=rwc",
        dir.path().join("gate.sqlite3").display()
    ))
    .await
    .unwrap();
    assert!(yorishiro::app::cli_timezone_gate_result(["app", "task", "sync"], None).is_err());
    migration::Migrator::up(&db, None).await.unwrap();
    let state = yorishiro::models::timezone_state::Entity::find_by_id(1)
        .one(&db)
        .await
        .unwrap()
        .expect("migration seeds the timezone singleton");
    assert_eq!(state.active_timezone, "UTC");
    assert!(
        yorishiro::app::cli_timezone_gate_result(
            ["app", "task", "sync"],
            Some(&state.active_timezone),
        )
        .is_ok()
    );
    db.close().await.unwrap();
}
