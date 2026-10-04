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

/// Caps applied when a tenant is on a given plan.
/// `max_workspaces` is written onto `tenant_tenants`; `default_max_entities` is the cap a caller should pass to `tenancy::create_workspace` for any workspace created while this plan is active.
/// Existing workspaces keep whatever cap they were created with, since retroactively shrinking a cap could put an existing workspace over its own limit.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlanCaps {
    pub(crate) max_workspaces: Option<i32>,
    pub(crate) default_max_entities: Option<i32>,
}

impl Plan {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Plan::Free => "free",
            Plan::Pro => "pro",
            Plan::Team => "team",
        }
    }

    /// Maps a Stripe Price id to the plan it represents.
    /// The mapping is configured via env vars rather than hardcoded, since Stripe price ids are specific to each Stripe account.
    pub(crate) fn from_stripe_price_id(
        price_id: &str,
        mapping: &StripePriceMapping,
    ) -> Option<Self> {
        if mapping.pro_price_id.as_deref() == Some(price_id) {
            Some(Plan::Pro)
        } else if mapping.team_price_id.as_deref() == Some(price_id) {
            Some(Plan::Team)
        } else {
            None
        }
    }

    pub(crate) fn caps(self) -> PlanCaps {
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

    pub(crate) fn from_db_str(value: &str) -> Result<Self, crate::YorishiroError> {
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

/// Which Stripe Price id corresponds to which plan, read from `YORISHIRO_STRIPE_PRICE_PRO`/`YORISHIRO_STRIPE_PRICE_TEAM`.
/// Both are `None` (no mapping) until an operator configures real Stripe price ids.
#[derive(Debug, Clone, Default)]
pub(crate) struct StripePriceMapping {
    pub(crate) pro_price_id: Option<String>,
    pub(crate) team_price_id: Option<String>,
}

impl StripePriceMapping {
    pub(crate) fn from_env() -> Self {
        Self {
            pro_price_id: crate::ee::data::non_empty_env("YORISHIRO_STRIPE_PRICE_PRO"),
            team_price_id: crate::ee::data::non_empty_env("YORISHIRO_STRIPE_PRICE_TEAM"),
        }
    }
}
