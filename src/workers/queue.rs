//! Queue scheduling policy shared by every Loco queue provider.
//!
//! The list of tags a worker process can request through `--worker=...` is not kept here: it is derived from [`crate::workers::registry::WorkerRegistry`].

use std::sync::Arc;

use loco_rs::app::AppContext;
use loco_rs::prelude::async_trait;
use uuid::Uuid;

use crate::workers::embedding_sync::WorkerClass;

/// The plan label this crate's own rule records on a lifecycle row.
const COMMUNITY_PLAN: &str = "community";

/// What a job is admitted under: its lifecycle plan label and an optional target concurrency cap.
pub struct ConcurrencyPolicy {
    pub plan: String,
    pub limit: Option<i32>,
}

/// Chooses an optional target admission cap and lifecycle plan label.
///
/// A seam: an edition that sells compute replaces this rule without touching the dispatcher that asks.
/// The error is a diagnostic for the operator, and a job whose policy cannot be determined is refused rather than admitted under a guess.
#[async_trait]
pub trait QueuePolicy: Send + Sync {
    async fn concurrency(
        &self,
        ctx: &AppContext,
        workspace_id: Uuid,
        class: WorkerClass,
    ) -> Result<ConcurrencyPolicy, String>;
}

/// Community admission has no target concurrency cap.
pub(crate) struct CommunityQueuePolicy;

#[async_trait]
impl QueuePolicy for CommunityQueuePolicy {
    async fn concurrency(
        &self,
        _ctx: &AppContext,
        _workspace_id: Uuid,
        _class: WorkerClass,
    ) -> Result<ConcurrencyPolicy, String> {
        Ok(ConcurrencyPolicy {
            plan: COMMUNITY_PLAN.to_owned(),
            limit: None,
        })
    }
}

/// The policy a deployment gets when it does not choose one.
pub fn default_queue_policy() -> Arc<dyn QueuePolicy> {
    Arc::new(CommunityQueuePolicy)
}

/// The concurrency policy `class` is admitted under for `workspace_id`, from whichever [`QueuePolicy`] is installed.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn concurrency_for(
    ctx: &AppContext,
    workspace_id: Uuid,
    class: WorkerClass,
) -> Result<ConcurrencyPolicy, String> {
    let policy = ctx
        .shared_store
        .get::<Arc<dyn QueuePolicy>>()
        .ok_or_else(|| "queue policy unavailable: no policy is installed".to_owned())?;
    policy.concurrency(ctx, workspace_id, class).await
}

const TENANT_PRIVATE_PRIORITY: i32 = 300;
const OFFICIAL_PRIORITY: i32 = 200;
const SHARED_PRIORITY: i32 = 100;

/// The provider priority band of each class: Loco dequeues larger values first, so the provider does the ordering.
pub const fn priority(class: WorkerClass) -> i32 {
    match class {
        WorkerClass::TenantPrivate => TENANT_PRIVATE_PRIORITY,
        WorkerClass::Official => OFFICIAL_PRIORITY,
        WorkerClass::Shared => SHARED_PRIORITY,
    }
}
