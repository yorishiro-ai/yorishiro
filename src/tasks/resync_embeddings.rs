use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::entity_embeddings;
use crate::models::entity_entities;
use crate::services::embedding;

/// `cargo loco task resync_embeddings workspace_id:<uuid>`
///
/// Re-syncs embeddings for entities that have no row in `entity_embeddings`: an operational recovery command for entities that fell out of search because no sync ever completed for them.
///
/// Two things leave an entity in that state. A sync that was enqueued but never succeeded (an embedding provider outage that outlasts the job's own retries), and a write that never enqueued one at all: `models::import`'s `import_jsonl` still does not, on either transport, so every entity restored from a backup needs this command run against its workspace before it is searchable by anything but the pg_trgm / FTS5 fuzzy fallback.
///
/// Uses a LEFT JOIN anti-join against `entity_embeddings` so the query works on both PostgreSQL and SQLite.
///
/// This calls `sync_embedding_for_record`, the same guarded path a normal entity write uses, deliberately: if the deployment's configured provider does not match a workspace's stamped model (`services/embedding/sync.rs`'s write-time model check), every candidate here fails for that reason and none get a vector.
/// That is correct, not a bug to route around: filling NULLs with vectors from a model the workspace is not stamped for would create the same silent model mix that check exists to prevent, just via this recovery path instead of an ordinary write.
/// `reindex_embeddings` is the tool for actually changing a workspace's model; this one is not, and must not be adapted into one.
pub(crate) struct ResyncEmbeddings;

#[async_trait]
impl Task for ResyncEmbeddings {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "resync_embeddings".to_string(),
            detail: "Re-syncs entities with no embedding: cargo loco task resync_embeddings workspace_id:<uuid>".to_string(),
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

        // An unconfigured embedding provider satisfies the dimension count but errors on every actual call.
        // Probe it once up front so a misconfiguration is one clear failure, not N per-candidate ones that read as an ordinary "N failed" outcome.
        let provider = app_context
            .shared_store
            .get::<std::sync::Arc<dyn embedding::EmbeddingProvider>>()
            .ok_or_else(|| {
                YorishiroError::Internal(anyhow::anyhow!("embedding provider missing"))
            })?;
        provider
            .embed_batch(&[])
            .await
            .map_err(|err| YorishiroError::ValidationFailed {
                message: format!("embedding provider must be configured: {err}"),
                details: vec![],
                hint: "check YORISHIRO_EMBEDDING_PROVIDER and YORISHIRO_LOCAL_MODEL config".into(),
            })?;

        let candidates = entity_entities::missing_embeddings(&app_context.db, workspace_id).await?;

        let mut synced = 0;
        let mut failed = 0;
        for candidate in &candidates {
            let result = entity_embeddings::sync_embedding_for_record(
                &app_context.db,
                workspace_id,
                candidate,
                provider.as_ref(),
            )
            .await;

            match result {
                Ok(()) => synced += 1,
                Err(err) => {
                    failed += 1;
                    eprintln!("  failed to resync entity {}: {err}", candidate.id);
                }
            }
        }

        println!(
            "resync finished: {} entities had no embedding, {synced} synced, {failed} failed \
             (entities whose entity_type has no x-embed field stay without embedding)",
            candidates.len(),
        );
        Ok(())
    }
}
