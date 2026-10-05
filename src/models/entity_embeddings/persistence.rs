use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

/// Input for writing an embedding vector with concurrency guarding.
pub(super) struct VectorWriteInput {
    pub workspace_id: Uuid,
    pub entity_id: Uuid,
    /// Monotonic token captured at the time the embedding vector was produced.
    /// `embed_and_write` only writes if the entity's current token equals this value,
    /// rejecting any write against an entity that was modified between capture and persist.
    pub embedding_sync_token: String,
    pub vector: Vec<f32>,
    pub dimension: usize,
}

/// Writes a vector to the width-specific table and guards it with the entity snapshot.
///
/// SeaORM cannot express `INSERT ... SELECT WHERE EXISTS ... ON CONFLICT ... DO UPDATE WHERE EXISTS ... RETURNING` (conditional upsert with a subquery in both INSERT and UPDATE branches, returning a count).
///
/// Returns `Ok(true)` when the write landed, `Ok(false)` when the entity was deleted,
/// and `Err(ConcurrentModification)` when the entity's token has changed since the
/// snapshot (meaning the embedding is stale and must be recomputed).
pub(super) async fn embed_and_write(
    conn: &impl ConnectionTrait,
    input: VectorWriteInput,
) -> Result<bool, YorishiroError> {
    let backend = conn.get_database_backend();

    let blob_bytes = if backend == DatabaseBackend::Sqlite {
        crate::db::sqlite_vec_blob(&input.vector)
    } else {
        Vec::new()
    };

    let table_name = super::embedding_table(input.dimension)?;
    let rows_affected = conn
        .execute_raw(Statement::from_sql_and_values(
            backend,
            format!(
                "INSERT INTO {table_name} (entity_id, embedding) \
                 SELECT $1, $2 \
                 WHERE EXISTS ( \
                   SELECT 1 FROM entity_entities \
                   WHERE entity_entities.id = $1 \
                     AND entity_entities.workspace_id = $3 \
                     AND entity_entities.embedding_sync_token = $4 \
                 ) \
                 ON CONFLICT(entity_id) \
                 DO UPDATE SET embedding = $2 \
                   WHERE {table_name}.entity_id = $1 \
                     AND EXISTS ( \
                       SELECT 1 FROM entity_entities \
                       WHERE entity_entities.id = {table_name}.entity_id \
                         AND entity_entities.workspace_id = $3 \
                         AND entity_entities.embedding_sync_token = $4 \
                     ) \
                 RETURNING 1"
            ),
            if backend == DatabaseBackend::Postgres {
                vec![
                    input.entity_id.into(),
                    sea_orm::entity::prelude::PgVector::from(input.vector).into(),
                    input.workspace_id.into(),
                    input.embedding_sync_token.into(),
                ]
            } else {
                vec![
                    input.entity_id.into(),
                    sea_orm::Value::from(blob_bytes),
                    input.workspace_id.into(),
                    input.embedding_sync_token.into(),
                ]
            },
        ))
        .await
        .internal()?;

    if rows_affected.rows_affected() == 0 {
        // The upsert found no matching entity row with the expected token.
        // Distinguish: was the entity deleted, or just token-mismatched?

        // Use query_one_raw instead of execute_raw(SELECT 1).rows_affected():
        // a SELECT never "affects" rows, so rows_affected is meaningless.
        let entity_exists = conn
            .query_one_raw(Statement::from_sql_and_values(
                backend,
                "SELECT 1 FROM entity_entities WHERE id = $1 AND workspace_id = $2",
                [input.entity_id.into(), input.workspace_id.into()],
            ))
            .await
            .internal()?
            .is_some();

        if entity_exists {
            return Err(YorishiroError::Conflict {
                message: format!(
                    "entity '{entity_id}' was modified while its embedding was being computed",
                    entity_id = input.entity_id
                ),
            });
        }

        tracing::info!(
            entity_id = %input.entity_id,
            "embed_and_write: entity was deleted, vector not stored"
        );
        return Ok(false);
    }

    Ok(true)
}
