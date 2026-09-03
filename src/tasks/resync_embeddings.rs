use loco_rs::prelude::*;
use loco_rs::task::Vars;
use sea_orm::{FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;

use crate::models::content_entities;
use crate::services::embedding;

/// `cargo loco task resync_embeddings workspace_id:<uuid>`
///
/// Re-syncs embeddings for entities that have no row in `content_entity_embeddings`: an operational recovery command for entities that fell out of search because no sync ever completed for them.
///
/// Two things leave an entity in that state. A sync that was enqueued but never succeeded (an embedding provider outage that outlasts the job's own retries), and a write that never enqueued one at all: `models::import`'s `import_jsonl` still does not, on either transport, so every entity restored from a backup needs this command run against its workspace before it is searchable by anything but the pg_trgm / FTS5 fuzzy fallback.
///
/// Uses a LEFT JOIN anti-join against `content_entity_embeddings` so the query works on both PostgreSQL and SQLite.
///
/// This calls `sync_embedding_for_record`, the same guarded path a normal entity write uses, deliberately: if the deployment's configured provider does not match a workspace's stamped model (`services/embedding/sync.rs`'s write-time model check), every candidate here fails for that reason and none get a vector.
/// That is correct, not a bug to route around: filling NULLs with vectors from a model the workspace is not stamped for would create the same silent model mix that check exists to prevent, just via this recovery path instead of an ordinary write.
/// `reindex_embeddings` is the tool for actually changing a workspace's model; this one is not, and must not be adapted into one.
pub struct ResyncEmbeddings;

#[derive(FromQueryResult, Clone)]
struct CandidateRow {
    id: Uuid,
    workspace_id: Uuid,
    schema_id: Uuid,
    schema_version: i32,
    entity_type: String,
    data: Value,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    created_by: Option<Uuid>,
    updated_by: Option<Uuid>,
}

impl From<CandidateRow> for content_entities::EntityRecord {
    fn from(row: CandidateRow) -> Self {
        content_entities::EntityRecord {
            id: row.id,
            workspace_id: row.workspace_id,
            schema_id: row.schema_id,
            schema_version: row.schema_version,
            entity_type: row.entity_type,
            data: row.data,
            created_at: row.created_at,
            updated_at: row.updated_at,
            created_by: row.created_by,
            updated_by: row.updated_by,
        }
    }
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
            .await
            .map_err(|err| Error::Message(format!("failed to build embedding provider: {err}")))?;
        provider.embed_batch(&[]).await.map_err(|err| {
            Error::Message(format!("embedding provider must be configured: {err}"))
        })?;

        // Single anti-join fetch: gets entity rows that have no embedding row, in one round trip instead of two.
        let candidates = CandidateRow::find_by_statement(Statement::from_sql_and_values(
            app_context.db.get_database_backend(),
            "SELECT e.id, e.workspace_id, e.schema_id, e.schema_version, \
             e.entity_type, e.data, e.created_at, e.updated_at, \
             e.created_by, e.updated_by \
             FROM content_entities e \
             LEFT JOIN content_entity_embeddings ee ON ee.entity_id = e.id \
             WHERE e.workspace_id = $1 AND ee.entity_id IS NULL",
            [workspace_id.into()],
        ))
        .all(&app_context.db)
        .await
        .map_err(|err| Error::Message(err.to_string()))?;

        let mut synced = 0;
        let mut failed = 0;
        for candidate in &candidates {
            let record = content_entities::EntityRecord {
                id: candidate.id,
                workspace_id: candidate.workspace_id,
                schema_id: candidate.schema_id,
                schema_version: candidate.schema_version,
                entity_type: candidate.entity_type.clone(),
                data: candidate.data.clone(),
                created_at: candidate.created_at,
                updated_at: candidate.updated_at,
                created_by: candidate.created_by,
                updated_by: candidate.updated_by,
            };
            let result = embedding::sync::sync_embedding_for_record(
                &app_context.db,
                workspace_id,
                &record,
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
