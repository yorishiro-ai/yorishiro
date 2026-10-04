use yorishiro::ee::data::plan::{ComputePolicy, Plan, PriorityTier};

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
fn burst_boundaries_require_demand_and_credits() {
    let policy = Plan::Pro.compute_policy();
    assert!(!is_eligible_burst(policy, 4, 1));
    assert!(is_eligible_burst(policy, 5, 1));
    assert!(is_eligible_burst(policy, 6, 1));
    assert!(!is_eligible_burst(policy, 7, 1));
    assert!(!is_eligible_burst(policy, 5, 0));
}

/// Burst applies only above the base concurrency, up to the ceiling, and while credits remain.
fn is_eligible_burst(
    policy: ComputePolicy,
    observed_official_demand: u32,
    available_credits: i64,
) -> bool {
    observed_official_demand > policy.base_official_concurrency
        && observed_official_demand <= policy.burst_ceiling
        && available_credits > 0
}
