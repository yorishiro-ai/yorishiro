//! Queue scheduling policy shared by every Loco queue provider.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::workers::embedding_sync::WorkerClass;

/// What a job is admitted under: the plan label recorded on its lifecycle row, and how many jobs of its class may run at once.
pub struct ConcurrencyPolicy {
    pub plan: String,
    pub limit: i32,
}

/// Decides how many jobs of a class one workspace may have running.
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

/// This crate's own rule: one running job per class, whatever the workspace.
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
            plan: "community".to_owned(),
            limit: 1,
        })
    }
}

/// The policy a deployment gets when it does not choose one.
pub fn default_queue_policy() -> Arc<dyn QueuePolicy> {
    Arc::new(CommunityQueuePolicy)
}

/// The concurrency policy `class` is admitted under for `workspace_id`, from whichever [`QueuePolicy`] is installed.
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

/// The named queue of every worker class, in the order a worker serving all of them should poll.
///
/// Redis honours a job's queue name and the SQL providers ignore it, so this is how a class gets a queue of its own on Redis only.
pub fn class_queues() -> Vec<String> {
    WorkerClass::ALL
        .iter()
        .map(|class| class.queue().to_owned())
        .collect()
}

const TENANT_PRIVATE_PRIORITY: i32 = 300;
const OFFICIAL_PRIORITY: i32 = 200;
const SHARED_PRIORITY: i32 = 100;

/// The provider priority bands used by all three supported Loco providers.
///
/// Loco defines larger values as more urgent and resolves ties by `run_at`, then
/// by the stable provider job id. Keeping a band per class means the provider
/// does the ordering, while this module owns the application policy.
pub const fn priority(class: WorkerClass) -> i32 {
    match class {
        WorkerClass::TenantPrivate => TENANT_PRIVATE_PRIORITY,
        WorkerClass::Official => OFFICIAL_PRIORITY,
        WorkerClass::Shared => SHARED_PRIORITY,
    }
}

pub const STARVATION_WAIT_SECONDS: i64 = 60;
pub const STARVATION_PRIORITY: i32 = 50;

/// A queue decision recorded for operators and deterministic unit tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub class: WorkerClass,
    pub priority: i32,
    pub fallback: bool,
}

/// Demotes a newly arriving higher class while an older lower class is waiting.
///
/// Loco does not expose a portable reprioritization operation for an already
/// queued job, so this boundary protects a waiting job by making subsequent
/// higher-class arrivals yield to it. The one-minute threshold and bounded
/// priority band make the result deterministic without a new queue provider.
pub(crate) async fn decide_for_dispatch(
    db: &sea_orm::DatabaseConnection,
    class: WorkerClass,
) -> Result<Decision, sea_orm::DbErr> {
    decide_for_dispatch_at(db, class, Utc::now().fixed_offset()).await
}

pub async fn decide_for_dispatch_at(
    db: &sea_orm::DatabaseConnection,
    class: WorkerClass,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> Result<Decision, sea_orm::DbErr> {
    let lower_classes: &[WorkerClass] = match class {
        WorkerClass::TenantPrivate => &[WorkerClass::Official, WorkerClass::Shared],
        WorkerClass::Official => &[WorkerClass::Shared],
        WorkerClass::Shared => &[],
    };
    if lower_classes.is_empty() {
        return Ok(decide(class));
    }
    let cutoff = now - chrono::Duration::seconds(STARVATION_WAIT_SECONDS);
    let waiting = crate::models::queue_job_lifecycles::Entity::count_waiting_lower_classes(
        db,
        lower_classes,
        cutoff,
    )
    .await?;
    Ok(if waiting > 0 {
        Decision {
            class,
            priority: STARVATION_PRIORITY,
            fallback: true,
        }
    } else {
        decide(class)
    })
}

/// Selects the initial class priority without consulting wall-clock time.
///
/// Capacity is deliberately not borrowed between classes. When a preferred
/// class is full, its job remains queued for that class instead of silently
/// consuming another class's reserved capacity. The independent class worker
/// is the starvation guard: work in another class can continue while this job
/// waits.
pub fn decide(class: WorkerClass) -> Decision {
    Decision {
        class,
        priority: priority(class),
        fallback: false,
    }
}
