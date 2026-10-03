//! The plan-driven rule for how many jobs of a class one workspace may run at once.

use async_trait::async_trait;
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::ee::controllers::middleware::edition::LicenceState;
use crate::ee::data::plan::Plan;
use crate::ee::models::billing;
use crate::error::YorishiroError;
use crate::models::tenancy;
use crate::workers::embedding_sync::WorkerClass;
use crate::workers::queue::{ConcurrencyPolicy, QueuePolicy};

/// An active licence names the plan outright; otherwise the tenant's billing row does, and a tenant with no row is on the free plan.
///
/// Only `Official` compute scales with the plan.
/// Every other class keeps one running job, since tenant-private and shared compute is not the deployment's to sell.
pub(crate) struct PlanQueuePolicy;

#[async_trait]
impl QueuePolicy for PlanQueuePolicy {
    async fn concurrency(
        &self,
        ctx: &AppContext,
        workspace_id: Uuid,
        class: WorkerClass,
    ) -> Result<ConcurrencyPolicy, String> {
        let plan = plan_for(ctx, workspace_id).await?;
        let limit = match class {
            WorkerClass::Official => plan.compute_policy().base_official_concurrency as i32,
            WorkerClass::TenantPrivate | WorkerClass::Shared => 1,
        };
        Ok(ConcurrencyPolicy {
            plan: plan.as_str().to_owned(),
            limit,
        })
    }
}

async fn plan_for(ctx: &AppContext, workspace_id: Uuid) -> Result<Plan, String> {
    let workspace = tenancy::get_workspace(&ctx.db, workspace_id)
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
    if let Some(plan) = licence.active_plan_at(chrono::Utc::now().timestamp()) {
        return Ok(plan);
    }

    let billing = billing::get_billing(&ctx.db, workspace.tenant_id)
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
