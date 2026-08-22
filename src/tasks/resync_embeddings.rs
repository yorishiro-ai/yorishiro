use loco_rs::prelude::*;
use loco_rs::task::Vars;
use sea_orm::{FromQueryResult, Statement};
use uuid::Uuid;

use crate::models::content_entities;
use crate::services::embedding;

/// `cargo loco task resync_embeddings workspace_id:<uuid>`
///
/// Re-syncs embeddings for entities whose `embedding` column is still NULL: an operational
/// recovery command for entities that fell out of search due to a failed background sync (a
/// transient embedding API outage, or the process exiting while `spawn_embedding_sync`'s task
/// was still in flight, see `controllers::entities`'s doc comment on that gap).
pub struct ResyncEmbeddings;

#[derive(Debug, FromQueryResult)]
struct CandidateId {
    id: Uuid,
}

#[async_trait]
impl Task for ResyncEmbeddings {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "resync_embeddings".to_string(),
            detail: "Re-syncs entities with no embedding: cargo loco task resync_embeddings workspace_id:<uuid>".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let workspace_id: Uuid = vars
            .cli_arg("workspace_id")?
            .parse()
            .map_err(|_| Error::Message("workspace_id is not a valid UUID".to_string()))?;

        // `build_embedding_provider` no longer fails when unconfigured (see its doc comment): it
        // falls back to `UnconfiguredEmbeddingProvider`, which satisfies the dimension count but
        // errors on every actual call. Left unchecked, every candidate below would fail
        // individually and the summary line would read as an ordinary "N failed" outcome instead
        // of the operator's actual problem (nothing is configured). One embed call up front,
        // before the loop, turns that into the same hard failure `build_embedding_provider`
        // itself used to be.
        let provider = embedding::build_embedding_provider()
            .map_err(|err| Error::Message(format!("failed to build embedding provider: {err}")))?;
        provider.embed_batch(&[]).await.map_err(|err| {
            Error::Message(format!("embedding provider must be configured: {err}"))
        })?;

        let candidates = CandidateId::find_by_statement(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT id FROM content_entities WHERE workspace_id = $1 AND embedding IS NULL",
            [workspace_id.into()],
        ))
        .all(&app_context.db)
        .await
        .map_err(|err| Error::Message(err.to_string()))?;

        let mut synced = 0;
        let mut failed = 0;
        for candidate in &candidates {
            let result = async {
                let record =
                    content_entities::get(&app_context.db, workspace_id, candidate.id).await?;
                embedding::sync::sync_embedding_for_record(
                    &app_context.db,
                    workspace_id,
                    &record,
                    provider.as_ref(),
                )
                .await
            }
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
