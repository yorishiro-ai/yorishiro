//! Background worker for workspace reindex operations.
//!
//! This worker re-embeds all entities in a workspace with the current provider model,
//! then restamps the workspace's `embedding_model`/`embedding_dimensions`.
//!
//! The REST endpoint (`POST /api/migration-jobs/reindex`) enqueues this worker,
//! which runs under the same advisory lock as the `reindex_embeddings` task,
//! so both entry points are serialized per workspace.

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::*;
use sea_orm::{FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::DbHandle;
use crate::services::embedding;

/// Arguments for the `reindex_worker` background worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReindexArgs {
    pub workspace_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct ReindexCandidateId {
    id: Uuid,
}

/// Background worker that runs a workspace reindex.
///
/// Reuses the same `reindex_workspace_with_lock` machinery as the `reindex_embeddings`
/// task, so both entry points share the advisory lock serialization and
/// restamp-on-full-success invariant.
///
/// This is the worker half of the REST endpoint: the endpoint enqueues this worker
/// and the dequeue side runs the same lock-and-reindex path.
///
/// Registered in `Hooks::connect_workers` under `class_name()` like the three
/// embedding sync workers: one `WorkerClass` per type, each tagged with its own.
pub struct ReindexWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<ReindexArgs> for ReindexWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    async fn perform(&self, args: ReindexArgs) -> loco_rs::Result<()> {
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
        let candidates = Self::fetch_candidates(&self.ctx.db, args.workspace_id).await?;
        let candidate_ids: Vec<Uuid> = candidates.iter().map(|c| c.id).collect();

        // Acquire the session-scoped lock and run the reindex.
        let db_handle = self.ctx.shared_store.get::<DbHandle>().ok_or_else(|| {
            loco_rs::Error::Message(
                "reindex requires the tenant pool, which this deployment did not build".into(),
            )
        })?;
        let outcome = crate::db::reindex_workspace_with_lock(
            db_handle.tenant.pool().clone(),
            args.workspace_id,
            &self.ctx.db,
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
}

impl ReindexWorker {
    /// Fetch all entity IDs for a workspace via raw SQL.
    ///
    /// PostgreSQL only: `content_entities` has no `embedding` column on SQLite.
    async fn fetch_candidates(
        db: &DatabaseConnection,
        workspace_id: Uuid,
    ) -> loco_rs::Result<Vec<ReindexCandidateId>> {
        let candidates: Vec<_> =
            ReindexCandidateId::find_by_statement(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT id FROM content_entities WHERE workspace_id = $1",
                [workspace_id.into()],
            ))
            .all(db)
            .await
            .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
        Ok(candidates)
    }
}

/// Enqueue a reindex job for `workspace_id` through Loco's queue.
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
    let args = serde_json::to_value(ReindexArgs { workspace_id })
        .map_err(|e| loco_rs::Error::Message(format!("serialize reindex args: {e}")))?;

    let job_id = ctx
        .queue_provider
        .as_ref()
        .ok_or_else(|| loco_rs::Error::Message("no queue provider configured".into()))?
        .enqueue(
            ReindexWorker::class_name(),
            ReindexWorker::queue(),
            args,
            None,
            None,
        )
        .await?
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    Ok(job_id)
}
