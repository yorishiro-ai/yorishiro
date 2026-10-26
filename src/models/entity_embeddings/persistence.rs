use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, ExprTrait, QueryFilter, SelectExt,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

/// Input for writing an embedding vector with concurrency guarding.
pub(super) struct VectorWriteInput {
    pub(super) workspace_id: Uuid,
    pub(super) entity_id: Uuid,
    /// Monotonic token captured at the time the embedding vector was produced.
    /// `embed_and_write` only writes if the entity's current token equals this value,
    /// rejecting any write against an entity that was modified between capture and persist.
    pub(super) embedding_sync_token: String,
    pub(super) vector: Vec<f32>,
}

/// Writes a vector and guards it with the entity snapshot.
///
/// Both insert and conflict-update branches check the current entity token in one SeaQuery statement.
///
/// Returns `Ok(true)` when the write landed, `Ok(false)` when the entity was deleted,
/// and `Err(ConcurrentModification)` when the entity's token has changed since the
/// snapshot (meaning the embedding is stale and must be recomputed).
pub(super) async fn embed_and_write(
    conn: &impl ConnectionTrait,
    input: VectorWriteInput,
) -> Result<bool, YorishiroError> {
    super::validate_vector(&input.vector, input.vector.len())?;
    let backend = conn.get_database_backend();
    let table = super::TABLE;

    use crate::models::_entities::entity_entities::{Column, Entity};
    use sea_orm::sea_query::{Alias, Expr, OnConflict, Query};

    let vector: sea_orm::Value = match backend {
        DatabaseBackend::Sqlite => crate::db::sqlite_vec_blob(&input.vector).into(),
        DatabaseBackend::Postgres => sea_orm::entity::prelude::PgVector::from(input.vector).into(),
        _ => {
            return Err(YorishiroError::BackendUnsupported {
                message: "embedding storage backend is unsupported".into(),
            });
        }
    };
    let table = Alias::new(table);
    let id = Alias::new("entity_id");
    let embedding = Alias::new("embedding");
    let current = Query::select()
        .expr(Expr::val(1))
        .from(Entity)
        .and_where(Expr::col(Column::Id).eq(input.entity_id))
        .and_where(Expr::col(Column::WorkspaceId).eq(input.workspace_id))
        .and_where(Expr::col(Column::EmbeddingSyncToken).eq(input.embedding_sync_token))
        .to_owned();
    let source = Query::select()
        .expr(Expr::val(input.entity_id))
        .expr(Expr::val(vector.clone()))
        .and_where(Expr::exists(current.clone()))
        .to_owned();
    let statement = Query::insert()
        .into_table(table.clone())
        .columns([id.clone(), embedding.clone()])
        .select_from(source)
        .map_err(|error| YorishiroError::Internal(error.into()))?
        .on_conflict(
            OnConflict::column(id.clone())
                .value(embedding, Expr::val(vector))
                .action_and_where(Expr::col((table, id)).eq(input.entity_id))
                .action_and_where(Expr::exists(current))
                .to_owned(),
        )
        .to_owned();
    let rows_affected = conn
        .execute_raw(backend.build(&statement))
        .await
        .internal()?;

    if rows_affected.rows_affected() == 0 {
        // The upsert found no matching entity row with the expected token.
        // Distinguish: was the entity deleted, or just token-mismatched?

        let entity_exists = Entity::find_by_id(input.entity_id)
            .filter(Column::WorkspaceId.eq(input.workspace_id))
            .exists(conn)
            .await
            .internal()?;

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
