use yorishiro::edition::ee::controllers::middleware::edition::{LicenceClaims, LicenceState};
use yorishiro::edition::ee::data::plan::Plan;

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
