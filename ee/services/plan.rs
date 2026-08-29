//! Subscription tiers for the hosted offering.
//!
//! Self-hosted deployments never assign a plan (`identity_tenants.plan` stays absent, since base never writes `identity_tenant_billing`); this type is only ever produced by this crate's Stripe integration.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    Free,
    Pro,
    Team,
}

/// Caps applied when a tenant is on a given plan.
/// `max_workspaces` is written straight onto `identity_tenants` (see `crate::models::tenancy::set_tenant_max_workspaces`); `default_max_entities` is the cap a caller should pass to `tenancy::create_workspace` for any workspace created while this plan is active.
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
