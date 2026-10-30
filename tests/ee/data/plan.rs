use yorishiro::edition::ee::controllers::middleware::edition::{LicenceClaims, LicenceState};
use yorishiro::edition::ee::data::plan::PlanPolicy;

#[test]
fn eligibility_requires_switch_and_current_licence() {
    let licence = LicenceState::licensed(LicenceClaims {
        sub: "policy-test".into(),
        plan: "pro".into(),
        exp: 100,
    });
    assert!(PlanPolicy::resolve(true, Some(&licence), 99) == PlanPolicy::Enabled);
    assert!(PlanPolicy::resolve(false, Some(&licence), 99) == PlanPolicy::Disabled);
    assert!(PlanPolicy::resolve(true, Some(&licence), 100) == PlanPolicy::Disabled);
    assert!(PlanPolicy::resolve(true, None, 99) == PlanPolicy::Disabled);
    assert!(PlanPolicy::resolve(true, Some(&LicenceState::default()), 99) == PlanPolicy::Disabled);
}
