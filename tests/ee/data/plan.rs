use crate::EnvGuard;
use serial_test::serial;
use yorishiro::edition::ee::data::plan::{Plan, PriorityTier, plan_policy_enabled};

#[tokio::test]
#[serial(process_environment)]
async fn admission_limits_follow_the_resolved_config_not_later_environment_changes() {
    use loco_rs::{app::Hooks, environment::Environment};
    use yorishiro::workers::embedding_sync::WorkerClass;
    let guard = EnvGuard::capture(&[
        "YORISHIRO_PLAN_POLICY_ENABLED",
        "YORISHIRO_WORKER_CLASS_SHARED_LIMIT",
    ]);
    guard.set("YORISHIRO_PLAN_POLICY_ENABLED", "true");
    guard.set("YORISHIRO_WORKER_CLASS_SHARED_LIMIT", "2");
    let directory = tempfile::tempdir().unwrap();
    let env = Environment::Any("test_sqlite".into());
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
    config
        .initializers
        .as_mut()
        .unwrap()
        .get_mut("worker_admission")
        .unwrap()["shared"] = 7.into();
    guard.set("YORISHIRO_WORKER_CLASS_SHARED_LIMIT", "invalid-after-load");
    guard.set("YORISHIRO_PLAN_POLICY_ENABLED", "invalid-after-load");
    let boot = yorishiro::App::boot(
        loco_rs::boot::StartMode::ServerOnly,
        &Environment::Test,
        config,
    )
    .await
    .unwrap();
    let ctx = &boot.app_context;
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    use yorishiro::models::_entities::{tenant_tenants, workspace_workspaces};
    let tenant = tenant_tenants::ActiveModel {
        name: Set("config-limits".into()),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .unwrap();
    let workspace = workspace_workspaces::ActiveModel {
        tenant_id: Set(tenant.id),
        name: Set("main".into()),
        status: Set("active".into()),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .unwrap();
    let policy = ctx
        .shared_store
        .get::<std::sync::Arc<dyn yorishiro::workers::queue::QueuePolicy>>()
        .unwrap();
    assert_eq!(
        policy
            .concurrency(ctx, workspace.id, WorkerClass::Shared)
            .await
            .unwrap()
            .limit,
        Some(7)
    );
    let path = directory.path().join("app.sqlite3");
    crate::requests::close_app_pools_sqlite(ctx, path.to_str().unwrap()).await;
}

#[test]
fn plan_claims_preserve_existing_values_and_map_enterprise_to_team() {
    let claims = [
        ("free", Plan::Free),
        ("pro", Plan::Pro),
        ("team", Plan::Team),
        ("enterprise", Plan::Team),
    ];
    for (claim, expected) in claims {
        let licensed =
            yorishiro::edition::ee::controllers::middleware::edition::LicenceState::licensed(
                yorishiro::edition::ee::controllers::middleware::edition::LicenceClaims {
                    sub: "test-customer".into(),
                    plan: claim.into(),
                    exp: 1000,
                },
            );
        assert_eq!(licensed.active_plan_at(999), Some(expected));
    }

    let unknown = yorishiro::edition::ee::controllers::middleware::edition::LicenceState::licensed(
        yorishiro::edition::ee::controllers::middleware::edition::LicenceClaims {
            sub: "test-customer".into(),
            plan: "unknown".into(),
            exp: 1000,
        },
    );
    assert_eq!(unknown.active_plan_at(999), None);
}

#[test]
fn compute_policies_match_the_published_bounds() {
    let free = Plan::Free.compute_policy();
    assert_eq!(free.priority, PriorityTier::Low);
    assert_eq!(free.queue_start_objective_seconds, None);
    assert_eq!((free.base_official_concurrency, free.burst_ceiling), (1, 2));
    let pro = Plan::Pro.compute_policy();
    assert_eq!(pro.priority, PriorityTier::Normal);
    assert_eq!(pro.queue_start_objective_seconds, Some(60));
    assert_eq!((pro.base_official_concurrency, pro.burst_ceiling), (4, 6));
    let team = Plan::Team.compute_policy();
    assert_eq!(team.priority, PriorityTier::High);
    assert_eq!(team.queue_start_objective_seconds, Some(15));
    assert_eq!(
        (team.base_official_concurrency, team.burst_ceiling),
        (8, 12)
    );
}

#[test]
fn plan_policy_parser_accepts_typed_and_legacy_text_values() {
    let mut config = crate::sqlite_fixture_config("sqlite::memory:");
    config.initializers = None;
    assert!(!plan_policy_enabled(&config).unwrap());
    for (value, expected) in [
        (serde_json::json!(true), true),
        (serde_json::json!("TrUe"), true),
        (serde_json::json!(false), false),
        (serde_json::json!("false"), false),
        (serde_json::json!(""), false),
    ] {
        config.initializers = Some(std::collections::BTreeMap::from([(
            "plan_policy_enabled".into(),
            value,
        )]));
        assert_eq!(plan_policy_enabled(&config).unwrap(), expected);
    }
    for value in [serde_json::json!("invalid"), serde_json::json!(1)] {
        config
            .initializers
            .as_mut()
            .unwrap()
            .insert("plan_policy_enabled".into(), value);
        let error = plan_policy_enabled(&config).expect_err("invalid flag");
        assert!(error.to_string().contains("YORISHIRO_PLAN_POLICY_ENABLED"));
    }
}

#[test]
#[serial(process_environment)]
fn enterprise_worker_limits_reject_invalid_values_at_startup() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "YORISHIRO_WORKER_CLASS_SHARED_LIMIT",
        "YORISHIRO_WORKER_CLASS_TENANT_PRIVATE_LIMIT",
        "YORISHIRO_WORKER_OFFICIAL_LIMIT",
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_yorishiro"))
            .args(["start", "--worker=worker-target:shared"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("LOCO_ENV", "test_sqlite")
            .env(
                "DATABASE_URL",
                format!(
                    "sqlite://{}?mode=rwc",
                    dir.path().join("app.sqlite3").display()
                ),
            )
            .env(
                "QUEUE_URL",
                format!(
                    "sqlite://{}?mode=rwc",
                    dir.path().join("queue.sqlite3").display()
                ),
            )
            .env(name, "0")
            .output()
            .expect("start worker with invalid limit");
        assert!(!output.status.success(), "{name} must be rejected");
        let message = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(message.contains(name), "{name}: {message}");
    }
}

#[tokio::test]
#[serial(process_environment)]
async fn invalid_plan_policy_fails_enterprise_boot() {
    struct StartupFiles {
        _directory: tempfile::TempDir,
        db: String,
        queue: String,
    }
    impl Drop for StartupFiles {
        fn drop(&mut self) {
            for path in [&self.db, &self.queue] {
                for suffix in ["", "-wal", "-shm", "-journal"] {
                    let _ = std::fs::remove_file(format!("{path}{suffix}"));
                }
            }
        }
    }

    let guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_TEST_TOPOLOGY",
        "YORISHIRO_PLAN_POLICY_ENABLED",
    ]);
    guard.set("YORISHIRO_TEST_TOPOLOGY", "sqlite-sqlite");
    use loco_rs::app::Hooks;
    let directory = tempfile::tempdir().expect("create startup test directory");
    let db = directory.path().join("db.sqlite3").display().to_string();
    let queue = directory.path().join("queue.sqlite3").display().to_string();
    let _files = StartupFiles {
        _directory: directory,
        db: db.clone(),
        queue: queue.clone(),
    };
    guard.set("DATABASE_URL", format!("sqlite://{db}?mode=rwc"));
    guard.set("QUEUE_URL", format!("sqlite://{queue}?mode=rwc"));
    guard.set("YORISHIRO_PLAN_POLICY_ENABLED", "false");
    let config = yorishiro::App::load_config(&loco_rs::environment::Environment::Test)
        .await
        .expect("load test config");
    let boot = yorishiro::App::boot(
        loco_rs::boot::StartMode::ServerOnly,
        &loco_rs::environment::Environment::Test,
        config,
    )
    .await
    .expect("false policy must boot");
    crate::requests::close_app_pools_sqlite(&boot.app_context, &db).await;

    guard.set("YORISHIRO_PLAN_POLICY_ENABLED", "invalid");
    let config = yorishiro::App::load_config(&loco_rs::environment::Environment::Test)
        .await
        .expect("load test config");
    let result = yorishiro::App::boot(
        loco_rs::boot::StartMode::ServerOnly,
        &loco_rs::environment::Environment::Test,
        config,
    )
    .await;
    assert!(result.is_err(), "invalid policy must fail boot");
}
