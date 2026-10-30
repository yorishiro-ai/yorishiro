//! The plan-driven rule for how many jobs of a class one workspace may run at once.

use loco_rs::app::AppContext;
use loco_rs::prelude::async_trait;
use std::sync::Arc;
use uuid::Uuid;

use crate::edition::ee::controllers::middleware::edition::LicenceState;
use crate::edition::ee::data::plan::{Plan, PlanPolicy};
use crate::edition::ee::models::tenant_billing;
use crate::error::YorishiroError;
use crate::models::workspace_workspaces;
use crate::workers::embedding_sync::WorkerClass;
use crate::workers::queue::{ConcurrencyPolicy, QueuePolicy};

/// Plan admission is enabled only by both the configuration switch and a current licence.
///
/// Plan selection remains part of the enterprise policy, while admission limits are deployment settings shared by all plans.
pub(crate) struct PlanQueuePolicy;

#[async_trait]
impl QueuePolicy for PlanQueuePolicy {
    async fn concurrency(
        &self,
        ctx: &AppContext,
        workspace_id: Uuid,
        class: WorkerClass,
    ) -> Result<ConcurrencyPolicy, String> {
        if PlanPolicy::current(ctx).map_err(|error| error.to_string())? == PlanPolicy::Disabled {
            return crate::workers::queue::default_queue_policy()
                .concurrency(ctx, workspace_id, class)
                .await;
        }
        let plan = plan_for(ctx, workspace_id).await?;
        let limits = ctx
            .shared_store
            .get::<Arc<crate::edition::ee::data::worker_policy::WorkerClassLimits>>()
            .ok_or_else(|| "queue policy unavailable: worker limits are missing".to_owned())?;
        let limit = Some(match class {
            WorkerClass::Shared => limits.shared,
            WorkerClass::TenantPrivate => limits.tenant_private,
            WorkerClass::Official => limits
                .official
                .unwrap_or_else(|| plan.compute_policy().base_official_concurrency as i32),
        });
        Ok(ConcurrencyPolicy {
            plan: plan.as_str().to_owned(),
            limit,
        })
    }
}

async fn plan_for(ctx: &AppContext, workspace_id: Uuid) -> Result<Plan, String> {
    let workspace = workspace_workspaces::get_workspace(&ctx.db, workspace_id)
        .await
        .map_err(|error| match error {
            YorishiroError::NotFound { .. } => {
                "queue policy unavailable: workspace does not exist".to_owned()
            }
            other => format!("queue policy lookup failed for workspace: {other}"),
        })?;

    let licence = ctx
        .shared_store
        .get::<std::sync::Arc<LicenceState>>()
        .ok_or_else(|| "queue policy unavailable: licence state is missing".to_owned())?;
    if let Some(plan) = licence.active_plan_at(crate::time::now().timestamp()) {
        return Ok(plan);
    }

    let billing = tenant_billing::get_billing(&ctx.db, workspace.tenant_id)
        .await
        .map_err(|error| format!("queue policy lookup failed for billing: {error}"))?;
    let Some(billing) = billing else {
        return Ok(Plan::Free);
    };
    let value = billing
        .plan
        .ok_or_else(|| "queue policy unavailable: billing plan is missing".to_owned())?;
    Plan::from_db_str(&value).map_err(|error| error.to_string())
}
