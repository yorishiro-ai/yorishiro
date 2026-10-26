//! Background worker for workspace reindex operations.
//!
//! This worker re-embeds all entities in a workspace with the current provider model,
//! then restamps the workspace's `embedding_model`/`embedding_dimensions`.
//!
//! The REST endpoint (`POST /api/migration-jobs/reindex`) enqueues this worker,
//! which runs under the same advisory lock as the `reindex_embeddings` task,
//! so both entry points are serialized per workspace.
//!
//! **Routing**: like `embedding_sync`, a reindex job carries one `worker-target:<name>` tag chosen when it is enqueued,
//! so `--worker=worker-target:tenant:<id>` picks up only that tenant's reindex jobs.

use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::services::embedding::EmbeddingProviderAvailability;
use crate::workers::embedding_sync::WorkerClass;

/// Arguments for a reindex worker job: workspace id and the resolved worker class tag.
///
/// `worker_class` determines which tag the job lands under in the queue, so that
/// tag-restricted worker processes dequeue only their own jobs.
#[derive(Clone, Serialize, Deserialize)]
pub struct ReindexArgs {
    #[serde(default)]
    pub lifecycle_id: Option<Uuid>,
    pub workspace_id: Uuid,
    pub worker_class: WorkerClass,
    #[serde(default)]
    pub startup: bool,
}

impl ReindexArgs {
    /// The name this job is recorded under in the queue lifecycle.
    pub(crate) const JOB_NAME: &'static str = "reindex";
    pub(crate) const STARTUP_JOB_NAME: &'static str = "startup_reindex";

    pub(crate) fn job_name(&self) -> &'static str {
        if self.startup {
            Self::STARTUP_JOB_NAME
        } else {
            Self::JOB_NAME
        }
    }
}

/// Shared implementation of the reindex worker's `perform` body: builds the provider,
/// fetches candidates, acquires the lock, and runs `reindex_workspace_with_lock`.
///
/// Called by [`ReindexWorker`] once the job has been admitted.
async fn perform_reindex(ctx: &AppContext, args: &ReindexArgs) -> loco_rs::Result<()> {
    // Resolve in the worker so workspace-specific overrides are honored without putting
    // credentials in the queue payload.
    let provider = crate::models::workspace_workspaces::embedding_provider(ctx, args.workspace_id)
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?
        .ok_or_else(|| loco_rs::Error::Message("embedding provider absent".into()))?;
    if provider.availability() == EmbeddingProviderAvailability::Disabled {
        return Err(loco_rs::Error::Message(
            "embedding provider disabled".into(),
        ));
    }
    let concurrency = ctx
        .shared_store
        .get::<std::sync::Arc<tokio::sync::Semaphore>>()
        .ok_or_else(|| loco_rs::Error::Message("embedding concurrency missing".into()))?;
    let _permit = concurrency
        .acquire_owned()
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    let outcome = crate::db::reindex_workspace_with_lock(ctx, args.workspace_id, provider.as_ref())
        .await
        .map_err(|e| loco_rs::Error::Message(e.to_string()))?;

    if !outcome.failures.is_empty() {
        return Err(loco_rs::Error::Message(format!(
            "reindex incomplete: {} entities, {} reindexed, {} failed",
            outcome.total,
            outcome.reindexed,
            outcome.failures.len()
        )));
    }

    Ok(())
}

/// The worker that reindexes one workspace.
///
/// It declares no tags and no queue: the durable dispatcher records the resolved target in the outbox.
pub struct ReindexWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<ReindexArgs> for ReindexWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    async fn perform(&self, args: ReindexArgs) -> loco_rs::Result<()> {
        crate::workers::lifecycle::perform_with_terminal_lifecycle::<Self, _, _, _>(
            &self.ctx,
            args.lifecycle_id,
            args.worker_class,
            args.workspace_id,
            &args,
            || perform_reindex(&self.ctx, &args),
        )
        .await
    }
}

/// Enqueues a reindex job through the durable outbox.
///
/// # Errors
/// Returns an error if the durable enqueue fails.
pub async fn enqueue_for_class(ctx: &AppContext, args: ReindexArgs) -> loco_rs::Result<()> {
    crate::workers::dispatch::dispatch_reindex(ctx, args, None)
        .await
        .map(|_| ())
}

/// Enqueues reindex in the caller transaction when supplied.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn enqueue_reindex(
    ctx: &AppContext,
    workspace_id: Uuid,
    txn: Option<&sea_orm::DatabaseTransaction>,
) -> loco_rs::Result<String> {
    if ctx.queue_provider.is_none() {
        return Err(loco_rs::Error::Message(
            "no queue provider configured".into(),
        ));
    }

    let worker_class = crate::controllers::extractors::resolve_worker_class(ctx, workspace_id)
        .await
        .map_err(|error| loco_rs::Error::from(error.0))?;
    crate::workers::dispatch::dispatch_reindex(
        ctx,
        ReindexArgs {
            lifecycle_id: None,
            workspace_id,
            worker_class,
            startup: false,
        },
        txn,
    )
    .await
}
