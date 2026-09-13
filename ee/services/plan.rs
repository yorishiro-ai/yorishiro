//! Subscription tiers for the hosted offering.
//!
//! Self-hosted deployments never assign a plan (`tenant_tenants.plan` stays absent, since base never writes `tenant_billing`); this type is only ever produced by this crate's Stripe integration.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    Free,
    Pro,
    Team,
}

/// Internal scheduling priority derived from the tenant's subscription plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityTier {
    Low,
    Normal,
    High,
}

/// Bounded compute policy used by the worker-class resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComputePolicy {
    pub priority: PriorityTier,
    pub queue_start_objective_seconds: Option<u32>,
    pub base_official_concurrency: u32,
    pub burst_ceiling: u32,
}

impl ComputePolicy {
    #[must_use]
    pub fn is_eligible_burst(self, observed_official_demand: u32, available_credits: i64) -> bool {
        observed_official_demand > self.base_official_concurrency
            && observed_official_demand <= self.burst_ceiling
            && available_credits > 0
    }
}

/// Caps applied when a tenant is on a given plan.
/// `max_workspaces` is written straight onto `tenant_tenants` (see `crate::models::tenancy::set_tenant_max_workspaces`); `default_max_entities` is the cap a caller should pass to `tenancy::create_workspace` for any workspace created while this plan is active.
/// Existing workspaces keep whatever cap they were created with, since retroactively shrinking a cap could put an existing workspace over its own limit.
#[derive(Debug, Clone, Copy)]
pub struct PlanCaps {
    pub max_workspaces: Option<i32>,
    pub default_max_entities: Option<i32>,
}

impl Plan {
    pub fn as_str(self) -> &'static str {
        match self {
            Plan::Free => "free",
            Plan::Pro => "pro",
            Plan::Team => "team",
        }
    }

    /// Maps a Stripe Price id to the plan it represents.
    /// The mapping is configured via env vars rather than hardcoded, since Stripe price ids are specific to each Stripe account.
    pub fn from_stripe_price_id(price_id: &str, mapping: &StripePriceMapping) -> Option<Self> {
        if mapping.pro_price_id.as_deref() == Some(price_id) {
            Some(Plan::Pro)
        } else if mapping.team_price_id.as_deref() == Some(price_id) {
            Some(Plan::Team)
        } else {
            None
        }
    }

    pub fn caps(self) -> PlanCaps {
        match self {
            Plan::Free => PlanCaps {
                max_workspaces: Some(1),
                default_max_entities: Some(500),
            },
            Plan::Pro => PlanCaps {
                max_workspaces: Some(5),
                default_max_entities: Some(50_000),
            },
            Plan::Team => PlanCaps {
                max_workspaces: None,
                default_max_entities: None,
            },
        }
    }

    /// Returns the bounded scheduling policy for this plan.
    #[must_use]
    pub fn compute_policy(self) -> ComputePolicy {
        let (priority, queue_start_objective_seconds, base_official_concurrency) = match self {
            Self::Free => (PriorityTier::Low, None, 1),
            Self::Pro => (PriorityTier::Normal, Some(60), 4),
            Self::Team => (PriorityTier::High, Some(15), 8),
        };
        ComputePolicy {
            priority,
            queue_start_objective_seconds,
            base_official_concurrency,
            burst_ceiling: base_official_concurrency + base_official_concurrency.div_ceil(2),
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, crate::YorishiroError> {
        match value {
            "free" => Ok(Self::Free),
            "pro" => Ok(Self::Pro),
            "team" => Ok(Self::Team),
            other => Err(crate::YorishiroError::Internal(anyhow::anyhow!(
                "unknown plan value: {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Plan, PriorityTier};

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
        assert!(!policy.is_eligible_burst(4, 1));
        assert!(policy.is_eligible_burst(5, 1));
        assert!(policy.is_eligible_burst(6, 1));
        assert!(!policy.is_eligible_burst(7, 1));
        assert!(!policy.is_eligible_burst(5, 0));
    }
}

/// Which Stripe Price id corresponds to which plan, read from `YORISHIRO_STRIPE_PRICE_PRO`/`YORISHIRO_STRIPE_PRICE_TEAM`.
/// Both are `None` (no mapping) until an operator configures real Stripe price ids.
#[derive(Debug, Clone, Default)]
pub struct StripePriceMapping {
    pub pro_price_id: Option<String>,
    pub team_price_id: Option<String>,
}

impl StripePriceMapping {
    pub fn from_env() -> Self {
        Self {
            pro_price_id: crate::ee::services::non_empty_env("YORISHIRO_STRIPE_PRICE_PRO"),
            team_price_id: crate::ee::services::non_empty_env("YORISHIRO_STRIPE_PRICE_TEAM"),
        }
    }
}
