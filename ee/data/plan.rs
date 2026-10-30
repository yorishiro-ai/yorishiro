//! Subscription tiers and licence-dependent plan-policy eligibility.

use serde::{Deserialize, Serialize};

/// Whether plan rules may be applied to the current operation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlanPolicy {
    Disabled,
    Enabled,
}

pub(crate) struct PlanPolicySetting {
    pub(crate) enabled: bool,
}

impl PlanPolicy {
    pub(crate) fn caps(self, plan: Plan) -> PlanCaps {
        match self {
            Self::Disabled => PlanCaps {
                max_workspaces: None,
                default_max_entities: None,
            },
            Self::Enabled => plan.caps(),
        }
    }

    /// Resolves the configured switch and current licence validity together.
    ///
    /// # Errors
    /// Returns an error if the deployment policy was not installed.
    pub fn current(ctx: &loco_rs::app::AppContext) -> Result<Self, crate::YorishiroError> {
        use crate::edition::ee::controllers::middleware::edition::LicenceState;
        use std::sync::Arc;

        let setting = ctx
            .shared_store
            .get::<Arc<PlanPolicySetting>>()
            .ok_or_else(|| {
                crate::YorishiroError::Internal(anyhow::anyhow!("plan policy missing"))
            })?;
        let licence = ctx.shared_store.get::<Arc<LicenceState>>();
        Ok(Self::resolve(
            setting.enabled,
            licence.as_deref(),
            crate::time::now().timestamp(),
        ))
    }

    /// Resolves eligibility at a specific clock boundary without freezing licence expiry.
    #[must_use]
    pub fn resolve(
        enabled: bool,
        licence: Option<&crate::edition::ee::controllers::middleware::edition::LicenceState>,
        now: i64,
    ) -> Self {
        if enabled && licence.is_some_and(|licence| licence.is_active_at(now)) {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    Free,
    Pro,
    Team,
}

/// Internal scheduling priority derived from the tenant's subscription plan.
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityTier {
    Low,
    Normal,
    High,
}

/// Bounded compute policy used by the worker-class resolver.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ComputePolicy {
    pub priority: PriorityTier,
    pub queue_start_objective_seconds: Option<u32>,
    pub base_official_concurrency: u32,
    pub burst_ceiling: u32,
}

/// Caps applied when a tenant is on a given plan.
/// `max_workspaces` is written onto `tenant_tenants`; `default_max_entities` is the cap a caller should pass to `tenancy::create_workspace` for any workspace created while this plan is active.
/// Existing workspaces keep whatever cap they were created with, since retroactively shrinking a cap could put an existing workspace over its own limit.
#[derive(Clone, Copy)]
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
            "enterprise" => Ok(Self::Team),
            other => Err(crate::YorishiroError::Internal(anyhow::anyhow!(
                "unknown plan value: {other:?}"
            ))),
        }
    }
}

/// Which Stripe Price id corresponds to which plan, read from `YORISHIRO_STRIPE_PRICE_PRO`/`YORISHIRO_STRIPE_PRICE_TEAM`.
/// Both are `None` (no mapping) until an operator configures real Stripe price ids.
#[derive(Clone, Default)]
pub(crate) struct StripePriceMapping {
    pub(crate) pro_price_id: Option<String>,
    pub(crate) team_price_id: Option<String>,
}

/// Reads and validates the enterprise plan-policy switch from resolved Loco initializer configuration.
///
/// # Errors
/// Returns an error when the value is neither a boolean nor its case-insensitive text representation.
pub fn plan_policy_enabled(
    config: &loco_rs::config::Config,
) -> Result<bool, crate::YorishiroError> {
    match config
        .initializers
        .as_ref()
        .and_then(|values| values.get("plan_policy_enabled"))
    {
        None | Some(serde_json::Value::Null) => Ok(false),
        Some(serde_json::Value::Bool(value)) => Ok(*value),
        Some(serde_json::Value::String(value))
            if value.is_empty() || value.eq_ignore_ascii_case("false") =>
        {
            Ok(false)
        }
        Some(serde_json::Value::String(value)) if value.eq_ignore_ascii_case("true") => Ok(true),
        Some(_) => {
            tracing::error!("invalid YORISHIRO_PLAN_POLICY_ENABLED; specify true or false");
            Err(crate::YorishiroError::Internal(anyhow::anyhow!(
                "YORISHIRO_PLAN_POLICY_ENABLED must be true or false"
            )))
        }
    }
}
