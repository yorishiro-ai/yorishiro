use yorishiro::ee::controllers::middleware::edition::{LicenceClaims, LicenceState};
use yorishiro::ee::data::plan::Plan;

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
fn active_licence_with_unknown_plan_falls_back() {
    let licensed = LicenceState::licensed(LicenceClaims {
        sub: "test-customer".into(),
        plan: "enterprise".into(),
        exp: 1000,
    });
    assert_eq!(licensed.active_plan_at(999), None);
}
