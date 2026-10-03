use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

pub(super) struct VectorWriteInput {
    pub workspace_id: Uuid,
    pub entity_id: Uuid,
    pub snapshot_updated_at: chrono::DateTime<chrono::Utc>,
    pub vector: Vec<f32>,
    pub dimension: usize,
}

/// Writes a vector to the width-specific table and guards it with the entity snapshot.
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

    let table_name = format!("entity_embeddings_{}", input.dimension);
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
                     AND entity_entities.updated_at = $4 \
                 ) \
                 ON CONFLICT(entity_id) \
                 DO UPDATE SET embedding = $2 \
                   WHERE {table_name}.entity_id = $1 \
                     AND EXISTS ( \
                       SELECT 1 FROM entity_entities \
                       WHERE entity_entities.id = {table_name}.entity_id \
                         AND entity_entities.workspace_id = $3 \
                         AND entity_entities.updated_at = $4 \
                     ) \
                 RETURNING 1"
            ),
            if backend == DatabaseBackend::Postgres {
                vec![
                    input.entity_id.into(),
                    sea_orm::entity::prelude::PgVector::from(input.vector).into(),
                    input.workspace_id.into(),
                    input.snapshot_updated_at.into(),
                ]
            } else {
                vec![
                    input.entity_id.into(),
                    sea_orm::Value::from(blob_bytes),
                    input.workspace_id.into(),
                    input.snapshot_updated_at.into(),
                ]
            },
        ))
        .await
        .internal()?;

    if rows_affected.rows_affected() == 0 {
        tracing::debug!(
            entity_id = %input.entity_id,
            "embed_and_write: entity was deleted or updated since this snapshot, write skipped"
        );
        return Ok(false);
    }

    Ok(true)
}
