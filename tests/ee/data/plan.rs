use crate::EnvGuard;
use serial_test::serial;
use yorishiro::edition::ee::data::plan::{Plan, PriorityTier, plan_policy_enabled};

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
#[serial(process_environment)]
fn plan_policy_parser_is_guarded_and_restores_environment() {
    let guard = EnvGuard::capture(&["YORISHIRO_PLAN_POLICY_ENABLED"]);
    guard.remove("YORISHIRO_PLAN_POLICY_ENABLED");
    assert!(!plan_policy_enabled().unwrap());
    guard.set("YORISHIRO_PLAN_POLICY_ENABLED", "TrUe");
    assert!(plan_policy_enabled().unwrap());
    guard.set("YORISHIRO_PLAN_POLICY_ENABLED", "false");
    assert!(!plan_policy_enabled().unwrap());
    guard.set("YORISHIRO_PLAN_POLICY_ENABLED", "invalid");
    let error = plan_policy_enabled().expect_err("invalid flag");
    assert!(error.to_string().contains("YORISHIRO_PLAN_POLICY_ENABLED"));
}

#[tokio::test]
#[serial(process_environment)]
async fn invalid_plan_policy_fails_enterprise_boot() {
    let guard = EnvGuard::capture(&[
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_TEST_TOPOLOGY",
        "YORISHIRO_PLAN_POLICY_ENABLED",
    ]);
    guard.set("YORISHIRO_TEST_TOPOLOGY", "sqlite-sqlite");
    use loco_rs::app::Hooks;
    let db = format!("/tmp/opencode/pr-f-533-boot-{}.sqlite3", std::process::id());
    let queue = format!(
        "/tmp/opencode/pr-f-533-boot-{}-queue.sqlite3",
        std::process::id()
    );
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
