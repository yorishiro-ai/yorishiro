use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::entity_entities;
use crate::workers::embedding_sync::{self, EmbeddingSyncArgs};

/// `cargo loco task resync_embeddings workspace_id:<uuid>`
///
/// Queues an embedding job for every entity that has no row in an embedding table: an operational recovery command for entities that fell out of search because no sync ever completed for them.
///
/// Two things leave an entity in that state. A sync that failed, because embedding is a single attempt and a failed job is not retried, and a write that never enqueued one at all: `models::import`'s `import_jsonl` still does not, on either transport, so every entity restored from a backup needs this command run against its workspace before it is searchable by anything but the pg_trgm / FTS5 fuzzy fallback.
///
/// The command only enqueues.
/// It holds no embedding provider, because only a worker process loads a model, so the worker's own guarded write path embeds each entity and the command's output counts queued jobs, not stored vectors.
/// That path refuses a vector from a model the workspace is not stamped for, which is correct, not a bug to route around: filling NULLs with vectors from another model would create the silent model mix that check exists to prevent.
/// `reindex_embeddings` is the tool for actually changing a workspace's model; this one is not, and must not be adapted into one.
///
/// Uses a LEFT JOIN anti-join against the embedding tables so the query works on both PostgreSQL and SQLite.
pub(crate) struct ResyncEmbeddings;

#[async_trait]
impl Task for ResyncEmbeddings {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "resync_embeddings".to_string(),
            detail: "Queues an embedding job for entities with no embedding: cargo loco task resync_embeddings workspace_id:<uuid>".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let workspace_id: Uuid = vars.cli_arg("workspace_id")?.parse().map_err(|_| {
            YorishiroError::ValidationFailed {
                message: "workspace_id is not a valid UUID".into(),
                details: vec![],
                hint: "workspace_id must be a UUID, e.g. 00000000-0000-0000-0000-000000000000"
                    .into(),
            }
        })?;

        let candidates = entity_entities::missing_embeddings(&app_context.db, workspace_id).await?;
        let worker_class =
            crate::controllers::extractors::resolve_worker_class(app_context, workspace_id)
                .await
                .map_err(|error| error.0)?;

        let mut queued = 0;
        let mut failed = 0;
        for candidate in &candidates {
            let job = EmbeddingSyncArgs {
                lifecycle_id: None,
                workspace_id,
                entity_id: candidate.id,
                worker_class,
            };
            match embedding_sync::enqueue_for_class(app_context, job).await {
                Ok(()) => queued += 1,
                Err(err) => {
                    failed += 1;
                    eprintln!("  failed to queue entity {}: {err}", candidate.id);
                }
            }
        }

        println!(
            "resync queued: {} entities had no embedding, {queued} queued for a worker, {failed} could not be queued \
             (entities whose entity_type has no x-embed field stay without embedding)",
            candidates.len(),
        );
        if failed > 0 {
            return Err(YorishiroError::Internal(anyhow::anyhow!(
                "{failed} of {} entities could not be queued",
                candidates.len()
            ))
            .into());
        }
        Ok(())
    }
}
