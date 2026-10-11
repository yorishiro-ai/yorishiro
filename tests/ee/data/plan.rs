use yorishiro::edition::ee::data::plan::{Plan, PriorityTier};

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
