use crate::services::plan::{Plan, StripePriceMapping};

#[test]
fn free_plan_caps_a_single_small_workspace() {
    let caps = Plan::Free.caps();
    assert_eq!(caps.max_workspaces, Some(1));
    assert_eq!(caps.default_max_entities, Some(500));
}

#[test]
fn team_plan_is_uncapped() {
    let caps = Plan::Team.caps();
    assert_eq!(caps.max_workspaces, None);
    assert_eq!(caps.default_max_entities, None);
}

#[test]
fn resolves_plan_from_configured_stripe_price_id() {
    let mapping = StripePriceMapping {
        pro_price_id: Some("price_pro_123".into()),
        team_price_id: Some("price_team_456".into()),
    };
    assert_eq!(
        Plan::from_stripe_price_id("price_pro_123", &mapping),
        Some(Plan::Pro)
    );
    assert_eq!(
        Plan::from_stripe_price_id("price_team_456", &mapping),
        Some(Plan::Team)
    );
    assert_eq!(Plan::from_stripe_price_id("price_unknown", &mapping), None);
}
