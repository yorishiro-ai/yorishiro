use loco_rs::prelude::*;
use loco_rs::task::Vars;
use sea_orm::{FromQueryResult, Statement};
use uuid::Uuid;

use crate::services::embedding;

/// `cargo loco task reindex_embeddings workspace_id:<uuid>`
///
/// Re-embeds every entity in a workspace with the deployment's currently configured embedding provider, then restamps `identity_workspaces.embedding_model`/`embedding_dimensions` to that provider's own values, so a workspace can move from one local model to another (nomic-embed-text-v1.5 to multilingual-e5-base, or any future model change) without the write-time model check in `services/embedding/sync.rs` refusing every subsequent write forever.
///
/// Unlike `resync_embeddings`, which only fills entities whose `embedding` column is NULL, this re-embeds every entity with `x-embed` fields regardless of whether it already has a vector: the whole point is replacing vectors from the old model, not filling gaps left by the old model.
///
/// The restamp happens only after every entity embeds successfully, never before and never partially; `embedding::sync::reindex_workspace` is where that ordering actually lives, and this task is a thin CLI shell over it.
/// Restamping first (or on partial success) would make the stamp claim a model that only some of the workspace's vectors actually came from, passing the write-time model check while the column itself still holds a mix: the exact failure this whole mechanism exists to prevent, just caused by the migration tool instead of an unconfigured deployment.
/// A failure partway through leaves the workspace stamped with its old model, which correctly keeps the write-time check refusing new writes until this task is re-run and succeeds; re-running is safe, since every entity is re-embedded again regardless of whether an earlier attempt already wrote a (partial, mixed) result.
///
/// An entity created or updated while a reindex is in flight goes through the ordinary guarded write path, which still checks against the workspace's (old, not yet restamped) stamp: it succeeds if it embeds with the old model and is refused if the deployment's provider has already moved to the new one, in which case it stays without a vector until `resync_embeddings` runs after this task's restamp completes.
///
/// This loads every candidate `EntityRecord` into memory in one batch, the same shape `resync_embeddings` already uses, rather than paging: consistent with that task, not a new consideration introduced here.
///
/// PostgreSQL only, for the same reason as `resync_embeddings`: `content_entities` has no `embedding` column at all on SQLite.
pub struct ReindexEmbeddings;

#[derive(Debug, FromQueryResult)]
struct CandidateId {
    id: Uuid,
}

#[async_trait]
impl Task for ReindexEmbeddings {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "reindex_embeddings".to_string(),
            detail: "Re-embeds every entity in a workspace with the current provider and restamps the workspace's model: cargo loco task reindex_embeddings workspace_id:<uuid>".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let workspace_id: Uuid = vars
            .cli_arg("workspace_id")?
            .parse()
            .map_err(|_| Error::Message("workspace_id is not a valid UUID".to_string()))?;

        // Mirrors resync_embeddings's own up-front probe: an unconfigured provider satisfies the
        // dimension count but errors on every actual call, so this turns that into one clear
        // failure instead of N per-entity ones that would read as an ordinary "N failed" outcome.
        let provider = embedding::build_embedding_provider()
            .await
            .map_err(|err| Error::Message(format!("failed to build embedding provider: {err}")))?;
        provider.embed_batch(&[]).await.map_err(|err| {
            Error::Message(format!("embedding provider must be configured: {err}"))
        })?;

        let candidates = CandidateId::find_by_statement(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT id FROM content_entities WHERE workspace_id = $1",
            [workspace_id.into()],
        ))
        .all(&app_context.db)
        .await
        .map_err(|err| Error::Message(err.to_string()))?;
        let candidate_ids: Vec<Uuid> = candidates.iter().map(|c| c.id).collect();

        let outcome = embedding::sync::reindex_workspace(
            &app_context.db,
            workspace_id,
            &candidate_ids,
            provider.as_ref(),
        )
        .await
        .map_err(|err| Error::Message(err.to_string()))?;

        if !outcome.failures.is_empty() {
            for failure in &outcome.failures {
                eprintln!(
                    "  failed to reindex entity {}: {}",
                    failure.entity_id, failure.error
                );
            }
            return Err(Error::Message(format!(
                "reindex incomplete: {} entities, {} reindexed, {} failed; the workspace's \
                 stamped model was left unchanged, so the write-time model check keeps refusing \
                 new writes until this task is re-run and every entity succeeds",
                outcome.total,
                outcome.reindexed,
                outcome.failures.len(),
            )));
        }

        println!(
            "reindex finished: {} entities, {} reindexed, workspace restamped to {:?} ({} \
             dimensions) (entities whose entity_type has no x-embed field stay without embedding)",
            outcome.total,
            outcome.reindexed,
            provider.model_name(),
            provider.dimensions(),
        );
        Ok(())
    }
}
