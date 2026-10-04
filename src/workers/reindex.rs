//! Background worker for workspace reindex operations.
//!
//! This worker re-embeds all entities in a workspace with the current provider model,
//! then restamps the workspace's `embedding_model`/`embedding_dimensions`.
//!
//! The REST endpoint (`POST /api/migration-jobs/reindex`) enqueues this worker,
//! which runs under the same advisory lock as the `reindex_embeddings` task,
//! so both entry points are serialized per workspace.
//!
//! **Tag routing**: like `embedding_sync`, each `WorkerClass` gets its own worker type
//! carrying a single fixed tag, so `--worker=worker-class:shared` etc. picks up only the
//! jobs that belong to that tag. Without this, a tag-restricted worker would dequeue zero
//! reindex jobs.

use std::sync::Arc;

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::DbHandle;
use crate::services::embedding;
use crate::workers::dispatch::ReindexDispatcher;
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
}

impl ReindexArgs {
    /// The name this job is recorded under in the queue lifecycle.
    pub(crate) const JOB_NAME: &'static str = "reindex";
}

/// Shared implementation of the reindex worker's `perform` body: builds the provider,
/// fetches candidates, acquires the lock, and runs `reindex_workspace_with_lock`.
///
/// Shared by all three worker types below, which differ only in the tag `tags()` returns.
async fn perform_reindex(ctx: &AppContext, args: &ReindexArgs) -> loco_rs::Result<()> {
    // Build and verify the provider: a reindex fails fast if the provider is
    // unconfigured, same as the task.
    let provider = ctx
        .shared_store
        .get::<std::sync::Arc<dyn embedding::EmbeddingProvider>>()
        .ok_or_else(|| loco_rs::Error::Message("embedding provider missing".into()))?;
    provider
        .embed_batch(&[])
        .await
        .map_err(|e| loco_rs::Error::Message(format!("provider must be configured: {e}")))?;

    // Fetch all entity IDs for this workspace.
    let candidate_ids =
        crate::models::entity_entities::ids_for_workspace(&ctx.db, args.workspace_id)
            .await
            .map_err(|e| loco_rs::Error::Message(e.to_string()))?;

    // Acquire the session-scoped lock and run the reindex.
    let db_handle = ctx.shared_store.get::<DbHandle>().ok_or_else(|| {
        loco_rs::Error::Message(
            "reindex requires the tenant pool, which this deployment did not build".into(),
        )
    })?;
    let outcome = crate::db::reindex_workspace_with_lock(
        db_handle.tenant.pool().clone(),
        args.workspace_id,
        &ctx.db,
        &candidate_ids,
        provider.as_ref(),
    )
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

/// Declares one `WorkerClass`'s reindex worker type: a thin struct giving `tags()` a fixed single tag,
/// so that class's reindex jobs are visible only to a worker process asking for it.
macro_rules! reindex_worker_for_class {
    ($worker_ty:ident, $class:expr) => {
        #[doc = concat!("`ReindexWorker` restricted to `", stringify!($class), "` jobs.")]
        pub struct $worker_ty {
            ctx: AppContext,
        }

        #[async_trait]
        impl BackgroundWorker<ReindexArgs> for $worker_ty {
            fn build(ctx: &AppContext) -> Self {
                Self { ctx: ctx.clone() }
            }

            fn tags() -> Vec<String> {
                vec![$class.tag().to_string()]
            }

            fn queue() -> Option<String> {
                Some($class.queue().to_string())
            }

            async fn perform(&self, args: ReindexArgs) -> loco_rs::Result<()> {
                crate::workers::lifecycle::perform_with_lifecycle::<Self, _, _, _>(
                    &self.ctx,
                    args.lifecycle_id,
                    $class,
                    &args,
                    || perform_reindex(&self.ctx, &args),
                )
                .await
            }
        }
    };
}

reindex_worker_for_class!(ReindexWorkerTenantPrivate, WorkerClass::TenantPrivate);
reindex_worker_for_class!(ReindexWorkerOfficial, WorkerClass::Official);
reindex_worker_for_class!(ReindexWorkerShared, WorkerClass::Shared);

/// Enqueues `args` on the worker type matching `args.worker_class`, so the queued tag
/// is the one the caller resolved.
///
/// Exhaustively matched, with no `_` arm: a fourth `WorkerClass` without its worker type
/// fails to compile rather than falling through to the wrong queue.
pub async fn enqueue_for_class(ctx: &AppContext, args: ReindexArgs) -> loco_rs::Result<()> {
    let dispatcher = ctx
        .shared_store
        .get::<Arc<dyn ReindexDispatcher>>()
        .ok_or_else(|| loco_rs::Error::Message("ReindexDispatcher missing".into()))?;
    enqueue_for_class_with_dispatcher(ctx, args, dispatcher.as_ref()).await
}

pub(crate) async fn enqueue_for_class_with_dispatcher(
    ctx: &AppContext,
    args: ReindexArgs,
    dispatcher: &dyn ReindexDispatcher,
) -> loco_rs::Result<()> {
    dispatcher.dispatch(ctx, args).await.map(|_job_id| ())
}

/// Enqueue a reindex job with a substituted dispatcher.
pub(crate) async fn enqueue_reindex_with_dispatcher(
    ctx: &AppContext,
    workspace_id: Uuid,
    dispatcher: &dyn ReindexDispatcher,
) -> loco_rs::Result<String> {
    if ctx.queue_provider.is_none() {
        return Err(loco_rs::Error::Message(
            "no queue provider configured".into(),
        ));
    }

    let worker_class = match crate::controllers::extractors::resolve_worker_class(ctx, workspace_id)
        .await
    {
        Ok(worker_class) => worker_class,
        Err(err) => {
            tracing::warn!(workspace_id = %workspace_id, error = %err.0, "failed to resolve worker class, defaulting to shared");
            WorkerClass::Shared
        }
    };
    dispatcher
        .dispatch(
            ctx,
            ReindexArgs {
                lifecycle_id: None,
                workspace_id,
                worker_class,
            },
        )
        .await
}

/// Enqueue a reindex job for `workspace_id` through Loco's queue.
pub(crate) async fn enqueue_reindex(
    ctx: &AppContext,
    workspace_id: Uuid,
) -> loco_rs::Result<String> {
    if ctx.queue_provider.is_none() {
        return Err(loco_rs::Error::Message(
            "no queue provider configured".into(),
        ));
    }
    let dispatcher = ctx
        .shared_store
        .get::<Arc<dyn ReindexDispatcher>>()
        .ok_or_else(|| loco_rs::Error::Message("ReindexDispatcher missing".into()))?;
    enqueue_reindex_with_dispatcher(ctx, workspace_id, dispatcher.as_ref()).await
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct RecordingDispatcher {
        args: Mutex<Vec<ReindexArgs>>,
        fail: bool,
    }

    #[async_trait]
    impl ReindexDispatcher for RecordingDispatcher {
        async fn dispatch(&self, _ctx: &AppContext, args: ReindexArgs) -> loco_rs::Result<String> {
            self.args.lock().unwrap().push(args);
            if self.fail {
                Err(loco_rs::Error::Message("dispatch failed".into()))
            } else {
                Ok("reindex-job".into())
            }
        }
    }

    #[tokio::test]
    async fn dispatches_the_original_args_and_returns_success() {
        let ctx = crate::workers::dispatch::test_context().await;
        let args = ReindexArgs {
            lifecycle_id: None,
            workspace_id: Uuid::now_v7(),
            worker_class: WorkerClass::TenantPrivate,
        };
        let dispatcher = RecordingDispatcher {
            args: Mutex::new(Vec::new()),
            fail: false,
        };

        enqueue_for_class_with_dispatcher(&ctx, args.clone(), &dispatcher)
            .await
            .expect("dispatch");

        let recorded = dispatcher.args.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].workspace_id, args.workspace_id);
        assert_eq!(recorded[0].worker_class, args.worker_class);
    }

    #[tokio::test]
    async fn preserves_dispatch_failure() {
        let ctx = crate::workers::dispatch::test_context().await;
        let dispatcher = RecordingDispatcher {
            args: Mutex::new(Vec::new()),
            fail: true,
        };

        let error = enqueue_for_class_with_dispatcher(
            &ctx,
            ReindexArgs {
                lifecycle_id: None,
                workspace_id: Uuid::now_v7(),
                worker_class: WorkerClass::Shared,
            },
            &dispatcher,
        )
        .await
        .expect_err("dispatch must fail");

        assert_eq!(error.to_string(), "dispatch failed");
    }

    #[tokio::test]
    async fn rejects_a_missing_queue_before_dispatch() {
        let ctx = crate::workers::dispatch::test_context().await;
        let dispatcher = RecordingDispatcher {
            args: Mutex::new(Vec::new()),
            fail: false,
        };

        let error = enqueue_reindex_with_dispatcher(&ctx, Uuid::now_v7(), &dispatcher)
            .await
            .expect_err("missing queue must fail");

        assert_eq!(error.to_string(), "no queue provider configured");
        assert!(dispatcher.args.lock().unwrap().is_empty());
    }
}
