//! Generates and stores an entity's embedding vector via Loco's own `BackgroundQueue`.
//!
//! Replaces the `tokio::spawn` this deployment used before a queue provider existed: that stand-in lost every in-flight sync on a process restart, a forced kill, or a provider outage past its own retry budget, leaving the entity's `embedding` column permanently `NULL` with nothing to retry it (see `tasks::resync_embeddings`'s doc comment for the operational recovery command that gap required).
//! `pg_loco_queue` persists the job in Postgres, so a re-deployed or restarted process resumes it instead of losing it.

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::content_entities;
use crate::services::embedding;

/// Only `workspace_id`/`entity_id`, not the `EntityRecord` itself: the record is re-read from the database inside `perform`, so a create-then-update (or create-then-delete) racing ahead of a still-queued job is picked up as the entity's current state rather than overwriting it with whatever was true at enqueue time.
/// A deleted entity is simply not found when re-read, and the job is a no-op for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingSyncArgs {
    pub workspace_id: Uuid,
    pub entity_id: Uuid,
}

pub struct EmbeddingSyncWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<EmbeddingSyncArgs> for EmbeddingSyncWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    /// Loco has no automatic retry for any queue driver (see `Queue::retry_failed`'s own doc comment): `Ok` marks the job `Completed` in `pg_loco_queue` and `Err` marks it `Failed`, and only `Failed` is something an operator can find and re-run with `retry_failed`.
    /// A structural failure (a schema no longer defining the field being embedded, a workspace's stamped dimension count not matching what the provider produces) will not go away on retry, so it is logged and reported `Ok`: retrying it would just mark the same job `Completed` again with nothing to show for the failure.
    /// A transient one (`ProviderBusy`, `ProviderUnreachable`, or `Internal`, chiefly a DB failure on the write/read this function itself does) is exactly what `Failed` plus `retry_failed` exists for, so those propagate as `Err`: reporting them `Ok` would let an entire provider outage's worth of jobs mark themselves `Completed` with the embedding still `NULL` and no record that anything went wrong, which is indistinguishable from a job that never needed to run at all.
    async fn perform(&self, args: EmbeddingSyncArgs) -> loco_rs::Result<()> {
        let provider = match crate::controllers::extractors::embedding_provider(&self.ctx) {
            Ok(provider) => provider,
            Err(err) => {
                tracing::warn!(entity_id = %args.entity_id, error = %err.0, "embedding sync worker: no embedding provider configured");
                return Ok(());
            }
        };

        let record = match content_entities::get(&self.ctx.db, args.workspace_id, args.entity_id)
            .await
        {
            Ok(record) => record,
            Err(crate::error::YorishiroError::NotFound { .. }) => {
                tracing::debug!(entity_id = %args.entity_id, "embedding sync worker: entity no longer exists, skipping");
                return Ok(());
            }
            Err(err) => {
                tracing::warn!(entity_id = %args.entity_id, error = %err, "embedding sync worker: failed to re-read entity");
                return Err(err.into());
            }
        };

        if let Err(err) = embedding::sync::sync_embedding_for_record(
            &self.ctx.db,
            args.workspace_id,
            &record,
            provider.as_ref(),
        )
        .await
        {
            use crate::error::YorishiroError;
            match err {
                YorishiroError::ProviderBusy { .. }
                | YorishiroError::ProviderUnreachable { .. }
                | YorishiroError::Internal(_) => {
                    tracing::warn!(entity_id = %args.entity_id, error = %err, "embedding sync failed transiently, job will be marked failed for retry_failed");
                    return Err(err.into());
                }
                YorishiroError::ValidationFailed { .. } | YorishiroError::NotFound { .. } => {
                    tracing::warn!(entity_id = %args.entity_id, error = %err, "embedding sync failed structurally, will not be retried");
                }
                other => {
                    // Every other variant reaches this path only via an unexpected future change to sync_embedding_for_record's error surface; treat as structural (not retried) rather than silently falling through, and the log line makes an unclassified variant visible instead of quietly swallowed.
                    tracing::warn!(entity_id = %args.entity_id, error = %other, "embedding sync failed with an unclassified error, treating as non-retryable");
                }
            }
        }

        Ok(())
    }
}
