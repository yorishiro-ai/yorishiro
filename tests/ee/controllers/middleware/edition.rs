mod gate;
mod licence;

use yorishiro::edition::ee::controllers::middleware::edition::{LicenceClaims, LicenceState};
use yorishiro::edition::ee::data::plan::Plan;

#[tokio::test]
#[serial_test::serial(process_environment)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "environment overrides remain active until this scope ends"
)]
async fn resolved_licence_settings_fail_closed_without_blocking_startup() {
    use loco_rs::app::Hooks;
    use serde_json::json;
    use std::sync::Arc;

    let directory = tempfile::tempdir().unwrap();

    let guard = crate::EnvGuard::capture(&["YORISHIRO_LICENSE_KEY", "DATABASE_URL", "QUEUE_URL"]);
    guard.set("YORISHIRO_LICENSE_KEY", "invalid-key-from-config-load");
    guard.set(
        "DATABASE_URL",
        format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("app.sqlite3").display()
        ),
    );
    guard.set(
        "QUEUE_URL",
        format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("queue.sqlite3").display()
        ),
    );
    let env = loco_rs::environment::Environment::Test;
    let mut config = yorishiro::App::load_config(&env).await.unwrap();
    assert_eq!(
        config.initializers.as_ref().unwrap()["licence_key"],
        "invalid-key-from-config-load"
    );
    guard.set("YORISHIRO_LICENSE_KEY", "changed-after-config-load");
    config.database.auto_migrate = false;
    config.database.dangerously_recreate = false;
    config.database.dangerously_truncate = false;
    for value in [
        json!("invalid-key-from-config-load"),
        json!(""),
        json!(null),
        json!(7),
    ] {
        config
            .initializers
            .as_mut()
            .unwrap()
            .insert("licence_key".into(), value.clone());
        let ctx = loco_rs::boot::create_context::<yorishiro::App>(&env, config.clone())
            .await
            .unwrap();
        assert_eq!(
            ctx.config.initializers.as_ref().unwrap()["licence_key"],
            value
        );
        assert!(
            !ctx.shared_store
                .get::<Arc<LicenceState>>()
                .unwrap()
                .is_active()
        );
        ctx.db.close().await.unwrap();
    }
}

#[test]
fn active_licence_plan_is_safe_and_expires_into_fallback() {
    let licensed = LicenceState::licensed(LicenceClaims {
        sub: "test-customer".into(),
        plan: "pro".into(),
        exp: 1000,
    });
    assert_eq!(licensed.active_plan_at(999), Some(Plan::Pro));
    assert_eq!(licensed.active_plan_at(1000), None);
}

#[test]
fn active_enterprise_licence_uses_team_policy() {
    let licensed = LicenceState::licensed(LicenceClaims {
        sub: "test-customer".into(),
        plan: "enterprise".into(),
        exp: 1000,
    });
    assert_eq!(licensed.active_plan_at(999), Some(Plan::Team));
    assert_eq!(Plan::Team.compute_policy().base_official_concurrency, 8);
}

/// Needs a real signed key, so it runs only in the CI job that holds the secret (`make test-licence`).
#[tokio::test]
async fn a_signed_licence_key_activates_enterprise_at_boot() {
    if std::env::var("YORISHIRO_LICENSE_KEY")
        .unwrap_or_default()
        .is_empty()
    {
        eprintln!("skipping: set YORISHIRO_LICENSE_KEY to verify a signed key");
        return;
    }
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let state = ctx
            .shared_store
            .get::<std::sync::Arc<LicenceState>>()
            .expect("production boot installs licence state");
        assert!(state.is_active(), "the signed key does not verify at boot");
    })
    .await;
}
