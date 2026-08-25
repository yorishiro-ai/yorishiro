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

/// Which class of worker process a queued job is meant for.
///
/// Routes jobs via `BackgroundWorker::tags()` (loco-rs's own dequeue-time tag filter, shared by the Postgres, `SQLite`, and Redis queue providers, unlike `queues: Option<Vec<String>>`, which only exists on the Redis config) rather than a named queue: this deployment's queue provider is Postgres (`pg_loco_queue`), and `queue: Option<String>` is silently discarded by that provider's `enqueue` (no column for it), so a named-queue split would require switching to Redis first.
/// Only `Shared` is actually produced today (see `enqueue_embedding_sync`'s call site in `controllers::entities`): the other two variants exist so `tags()` has something to route once a deployment can register a worker process that only wants tenant-private or official-node jobs, work this enum does not itself perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerClass {
    /// Runs only on compute a single tenant registered for its own workspaces.
    TenantPrivate,
    /// Runs only on compute this deployment operates itself.
    Official,
    /// Runs on any worker process willing to take the job; the default for a deployment with no registered compute of its own.
    Shared,
}

impl WorkerClass {
    /// The `tags()` value this class routes through.
    ///
    /// `worker-class:<variant>` rather than the bare variant name: a future tag dimension (region, priority band) added to the same job would otherwise collide on an unprefixed string with no way to tell which dimension it came from.
    fn tag(self) -> &'static str {
        match self {
            Self::TenantPrivate => "worker-class:tenant-private",
            Self::Official => "worker-class:official",
            Self::Shared => "worker-class:shared",
        }
    }
}

/// Only `workspace_id`/`entity_id`, not the `EntityRecord` itself: the record is re-read from the database inside `perform`, so a create-then-update (or create-then-delete) racing ahead of a still-queued job is picked up as the entity's current state rather than overwriting it with whatever was true at enqueue time.
/// A deleted entity is simply not found when re-read, and the job is a no-op for it.
///
/// No `model` field: `perform` already re-resolves the embedding provider from `workspace_id` via `WorkspaceEmbeddingResolver` on every run (the same live-lookup reasoning as not re-reading the entity at enqueue time), and `EmbeddingProvider` exposes no model identifier for a payload field to even mirror. Carrying one here would be a value nothing reads and nothing keeps in sync with the resolver's own answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingSyncArgs {
    pub workspace_id: Uuid,
    pub entity_id: Uuid,
    pub worker_class: WorkerClass,
}

pub struct EmbeddingSyncWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<EmbeddingSyncArgs> for EmbeddingSyncWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    /// A worker process only dequeues jobs matching at least one of these tags (see `pg.rs`'s `dequeue`, shared across the Postgres/`SQLite` queue providers).
    /// Every `WorkerClass` variant's tag is listed, not just `Shared`: this process is the only registered `EmbeddingSyncWorker`, so until a deployment can run a second, differently-tagged process (the not-yet-built external-node registration), one process must still take every class of job or `TenantPrivate`/`Official` jobs would queue forever with nothing to dequeue them.
    fn tags() -> Vec<String> {
        vec![
            WorkerClass::TenantPrivate.tag().to_string(),
            WorkerClass::Official.tag().to_string(),
            WorkerClass::Shared.tag().to_string(),
        ]
    }

    /// Loco has no automatic retry for any queue driver (see `Queue::retry_failed`'s own doc comment): `Ok` marks the job `Completed` in `pg_loco_queue` and `Err` marks it `Failed`, and only `Failed` is something an operator can find and re-run with `retry_failed`.
    /// A structural failure (a schema no longer defining the field being embedded, a workspace's stamped dimension count not matching what the provider produces) will not go away on retry, so it is logged and reported `Ok`: retrying it would just mark the same job `Completed` again with nothing to show for the failure.
    /// A transient one (`ProviderBusy`, `ProviderUnreachable`, or `Internal`, chiefly a DB failure on the write/read this function itself does) is exactly what `Failed` plus `retry_failed` exists for, so those propagate as `Err`: reporting them `Ok` would let an entire provider outage's worth of jobs mark themselves `Completed` with the embedding still `NULL` and no record that anything went wrong, which is indistinguishable from a job that never needed to run at all.
    async fn perform(&self, args: EmbeddingSyncArgs) -> loco_rs::Result<()> {
        let provider = match crate::controllers::extractors::resolve_embedding_provider(
            &self.ctx,
            args.workspace_id,
        )
        .await
        {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `EmbeddingSyncWorker::tags()` must list every `WorkerClass`'s own tag: it is the one process registered today, so a class this list drops would queue jobs no running process ever dequeues.
    #[test]
    fn worker_tags_cover_every_worker_class() {
        let worker_tags = EmbeddingSyncWorker::tags();
        for class in [
            WorkerClass::TenantPrivate,
            WorkerClass::Official,
            WorkerClass::Shared,
        ] {
            assert!(
                worker_tags.contains(&class.tag().to_string()),
                "tags() is missing {class:?}'s tag ({})",
                class.tag()
            );
        }
    }

    /// `serde(rename_all = "snake_case")` is what `EmbeddingSyncArgs` actually persists into `pg_loco_queue`'s `task_data`; asserting the wire form catches an accidental rename breaking a job already sitting in the queue at deploy time.
    #[test]
    fn worker_class_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_value(WorkerClass::TenantPrivate).unwrap(),
            serde_json::json!("tenant_private")
        );
        assert_eq!(
            serde_json::to_value(WorkerClass::Official).unwrap(),
            serde_json::json!("official")
        );
        assert_eq!(
            serde_json::to_value(WorkerClass::Shared).unwrap(),
            serde_json::json!("shared")
        );
    }
}
