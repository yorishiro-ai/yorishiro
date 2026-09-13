//! This crate's `WorkerClassResolver`: a workspace with its own row in `workspace_worker_classes` pins its embedding-sync jobs to that class instead of `WorkerClass::Shared`.

use crate::ee::models::billing;
use crate::ee::services::plan::{ComputePolicy, Plan};
use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_workspaces;
use crate::workers::embedding_sync::{WorkerClass, WorkerClassResolver};
use async_trait::async_trait;
use sea_orm::EntityTrait;
use uuid::Uuid;

use crate::ee::models::worker_classes;

pub struct WorkerClassAssignmentResolver;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteRecommendation {
    Official,
    Shared,
}

#[async_trait]
pub trait WasmOffloadHook: Send + Sync {
    async fn recommend(
        &self,
        workspace_id: Uuid,
        policy: ComputePolicy,
        observed_official_demand: u32,
        available_credits: i64,
    ) -> Result<RouteRecommendation, YorishiroError>;
}

pub struct NoopWasmOffloadHook;

#[async_trait]
impl WasmOffloadHook for NoopWasmOffloadHook {
    async fn recommend(
        &self,
        _workspace_id: Uuid,
        _policy: ComputePolicy,
        _observed_official_demand: u32,
        _available_credits: i64,
    ) -> Result<RouteRecommendation, YorishiroError> {
        Ok(RouteRecommendation::Shared)
    }
}

#[must_use]
pub fn route_for(
    explicit: Option<WorkerClass>,
    policy: ComputePolicy,
    recommendation: RouteRecommendation,
    observed_official_demand: u32,
    available_credits: i64,
) -> WorkerClass {
    if let Some(worker_class) = explicit {
        return worker_class;
    }
    if recommendation == RouteRecommendation::Official
        && policy.is_eligible_burst(observed_official_demand, available_credits)
    {
        WorkerClass::Official
    } else {
        WorkerClass::Shared
    }
}

#[async_trait]
impl WorkerClassResolver for WorkerClassAssignmentResolver {
    async fn resolve(
        &self,
        conn: &sea_orm::DatabaseConnection,
        workspace_id: Uuid,
    ) -> Result<Option<WorkerClass>, YorishiroError> {
        let explicit = worker_classes::get(conn, workspace_id).await?;
        if explicit.is_some() {
            return Ok(explicit);
        }
        let workspace = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(conn)
            .await
            .internal()?
            .ok_or_else(|| YorishiroError::not_found("workspace was not found"))?;
        let plan = billing::get_billing(conn, workspace.tenant_id)
            .await?
            .and_then(|record| record.plan)
            .map(|value| Plan::from_db_str(&value))
            .transpose()?
            .unwrap_or(Plan::Free);
        let policy = plan.compute_policy();
        let recommendation = NoopWasmOffloadHook
            .recommend(workspace_id, policy, 0, 0)
            .await?;
        match route_for(None, policy, recommendation, 0, 0) {
            WorkerClass::Official => Ok(Some(WorkerClass::Official)),
            WorkerClass::Shared => Ok(None),
            WorkerClass::TenantPrivate => {
                unreachable!("implicit routing never selects tenant private")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RouteRecommendation, route_for};
    use crate::ee::services::plan::Plan;
    use crate::workers::embedding_sync::WorkerClass;

    #[test]
    fn explicit_assignment_wins_over_every_policy_recommendation() {
        assert_eq!(
            route_for(
                Some(WorkerClass::TenantPrivate),
                Plan::Team.compute_policy(),
                RouteRecommendation::Official,
                12,
                1,
            ),
            WorkerClass::TenantPrivate
        );
    }

    #[test]
    fn official_requires_recommendation_burst_and_credits() {
        let policy = Plan::Pro.compute_policy();
        assert_eq!(
            route_for(None, policy, RouteRecommendation::Shared, 5, 1),
            WorkerClass::Shared
        );
        assert_eq!(
            route_for(None, policy, RouteRecommendation::Official, 4, 1),
            WorkerClass::Shared
        );
        assert_eq!(
            route_for(None, policy, RouteRecommendation::Official, 5, 0),
            WorkerClass::Shared
        );
        assert_eq!(
            route_for(None, policy, RouteRecommendation::Official, 5, 1),
            WorkerClass::Official
        );
        assert_eq!(
            route_for(None, policy, RouteRecommendation::Official, 7, 1),
            WorkerClass::Shared
        );
    }
}
