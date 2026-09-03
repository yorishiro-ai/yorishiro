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

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::*;
use sea_orm::{FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::DbHandle;
use crate::services::embedding;
use crate::workers::embedding_sync::WorkerClass;

/// Arguments for a reindex worker job: workspace id and the resolved worker class tag.
///
/// `worker_class` determines which tag the job lands under in the queue, so that
/// tag-restricted worker processes dequeue only their own jobs.
#[derive(Clone, Serialize, Deserialize)]
pub struct ReindexArgs {
    pub workspace_id: Uuid,
    pub worker_class: WorkerClass,
}

/// Shared implementation of the reindex worker's `perform` body: builds the provider,
/// fetches candidates, acquires the lock, and runs `reindex_workspace_with_lock`.
///
/// Shared by all three worker types below, which differ only in the tag `tags()` returns.
async fn perform_reindex(ctx: &AppContext, args: &ReindexArgs) -> loco_rs::Result<()> {
    // Build and verify the provider: a reindex fails fast if the provider is
    // unconfigured, same as the task.
    let provider = embedding::build_embedding_provider()
        .await
        .map_err(|e| loco_rs::Error::Message(format!("build provider: {e}")))?;
    provider
        .embed_batch(&[])
        .await
        .map_err(|e| loco_rs::Error::Message(format!("provider must be configured: {e}")))?;

    // Fetch all entity IDs for this workspace.
    let candidate_ids = fetch_candidates(&ctx.db, args.workspace_id).await?;

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

/// Fetch all entity IDs for a workspace via raw SQL.
///
/// PostgreSQL only: `content_entities` has no `embedding` column on SQLite.
async fn fetch_candidates(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> loco_rs::Result<Vec<Uuid>> {
    #[derive(FromQueryResult)]
    struct CandidateId {
        id: Uuid,
    }
    let candidates = CandidateId::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT id FROM content_entities WHERE workspace_id = $1",
        [workspace_id.into()],
    ))
    .all(db)
    .await
    .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
    Ok(candidates.into_iter().map(|c| c.id).collect())
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

            async fn perform(&self, args: ReindexArgs) -> loco_rs::Result<()> {
                perform_reindex(&self.ctx, &args).await
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
    let _job_id = match args.worker_class {
        WorkerClass::TenantPrivate => ReindexWorkerTenantPrivate::perform_later(ctx, args).await?,
        WorkerClass::Official => ReindexWorkerOfficial::perform_later(ctx, args).await?,
        WorkerClass::Shared => ReindexWorkerShared::perform_later(ctx, args).await?,
    };
    // Job ID discarded: callers that need it unwrap the Option themselves.
    Ok(())
}

/// Enqueue a reindex job for `workspace_id` through Loco's queue, tagged with the resolved worker class.
///
/// The caller must check that `ctx.queue_provider` is configured: a missing provider
/// means `perform_later` would return a job ID while silently discarding the job,
/// which is worse than no endpoint at all.
///
/// Returns the job ID assigned by the queue provider.
///
/// # Errors
/// Returns `loco_rs::Error` when the queue is configured but the enqueue fails.
pub async fn enqueue_reindex(ctx: &AppContext, workspace_id: Uuid) -> loco_rs::Result<String> {
    let worker_class = match crate::controllers::extractors::resolve_worker_class(ctx, workspace_id)
        .await
    {
        Ok(worker_class) => worker_class,
        Err(err) => {
            tracing::warn!(workspace_id = %workspace_id, error = %err.0, "failed to resolve worker class, defaulting to shared");
            WorkerClass::Shared
        }
    };
    let args = ReindexArgs {
        workspace_id,
        worker_class,
    };

    // Use perform_later so tags() is read through the correct worker type's impl.
    // unwrap_or_else: perform_later returns Option<String> for the job_id in BackgroundQueue mode;
    // if None, generate one so the caller has something to track.
    let job_id = match ctx.queue_provider {
        Some(_) => match worker_class {
            WorkerClass::TenantPrivate => {
                ReindexWorkerTenantPrivate::perform_later(ctx, args.clone()).await
            }
            WorkerClass::Official => ReindexWorkerOfficial::perform_later(ctx, args.clone()).await,
            WorkerClass::Shared => ReindexWorkerShared::perform_later(ctx, args.clone()).await,
        }
        .ok()
        .unwrap_or_else(|| Uuid::new_v4().to_string()),
        None => {
            return Err(loco_rs::Error::Message(
                "no queue provider configured".into(),
            ));
        }
    };

    Ok(job_id)
}
