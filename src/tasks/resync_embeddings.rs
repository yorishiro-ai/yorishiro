use loco_rs::prelude::*;
use loco_rs::task::Vars;
use sea_orm::{FromQueryResult, Statement};
use uuid::Uuid;

use crate::models::content_entities;
use crate::services::embedding;

/// `cargo loco task resync_embeddings workspace_id:<uuid>`
///
/// Re-syncs embeddings for entities whose `embedding` column is still NULL: an operational recovery command for entities that fell out of search due to a failed background sync (a transient embedding API outage, or the process exiting while `spawn_embedding_sync`'s task was still in flight, see `controllers::entities`'s doc comment on that gap).
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

        // An unconfigured embedding provider satisfies the dimension count but errors on every actual call.
        // Probe it once up front so a misconfiguration is one clear failure, not N per-candidate ones that read as an ordinary "N failed" outcome.
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

        // One query for every candidate's current row, instead of one per candidate: the
        // embedding-provider call and the UPDATE it triggers still happen per entity below (an
        // external HTTP call and a per-row write, neither of which batches the same way), but the
        // read that feeds them does not need its own round trip per row.
        let candidate_ids: Vec<Uuid> = candidates.iter().map(|c| c.id).collect();
        let records = content_entities::get_batch(&app_context.db, workspace_id, &candidate_ids)
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        let mut synced = 0;
        let mut failed = 0;
        for candidate in &candidates {
            let Some(record) = records.get(&candidate.id) else {
                // Deleted between the candidate scan above and this batch fetch.
                failed += 1;
                eprintln!("  entity {} no longer exists", candidate.id);
                continue;
            };
            let result = embedding::sync::sync_embedding_for_record(
                &app_context.db,
                workspace_id,
                record,
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
