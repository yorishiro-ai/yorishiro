//! Finding the entities `infer_fill` should consider.
//!
//! `infer_fill` writes a model's guess straight into `content_entities`, the same "compute and write immediately" shape `EmbeddingSyncWorker` uses: no separate proposal/confirm step, since a guess is reversible the same way any other write is, through base's own `content_entities::snapshot`/`undo_job` (`POST /api/migration-jobs/{job_id}/undo`).

use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect};
use serde_json::Value;
use uuid::Uuid;
use yorishiro_core::error::{ResultExt, YorishiroError};
use yorishiro_core::models::_entities::content_entities as content_entities_entity;
use yorishiro_core::models::_entities::content_schemas;

/// One entity `infer_fill` should consider: still on some earlier version of `name`, not yet on `active_schema_id`.
pub struct OutdatedEntity {
    pub id: Uuid,
    pub entity_type: String,
    pub data: Value,
}

/// Every entity in `workspace_id` still on a version of the schema named `name` other than `active_schema_id`.
/// The set `infer_fill` walks: an entity already on the active version has nothing the schema says is missing.
///
/// `content_schemas` rows sharing `(workspace_id, name)` are versions of the same logical schema, so this first
/// collects every version's id, then filters `content_entities` by membership in that set: an entity-API
/// equivalent of the join, since neither table needs a column the other doesn't already expose.
pub async fn entities_on_outdated_schema(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    name: &str,
    active_schema_id: Uuid,
) -> Result<Vec<OutdatedEntity>, YorishiroError> {
    let schema_ids: Vec<Uuid> = content_schemas::Entity::find()
        .filter(content_schemas::Column::WorkspaceId.eq(workspace_id))
        .filter(content_schemas::Column::Name.eq(name))
        .select_only()
        .column(content_schemas::Column::Id)
        .into_tuple()
        .all(conn)
        .await
        .internal()?;

    let rows = content_entities_entity::Entity::find()
        .filter(content_entities_entity::Column::WorkspaceId.eq(workspace_id))
        .filter(content_entities_entity::Column::SchemaId.is_in(schema_ids))
        .filter(content_entities_entity::Column::SchemaId.ne(active_schema_id))
        .all(conn)
        .await
        .internal()?;

    Ok(rows
        .into_iter()
        .map(|row| OutdatedEntity {
            id: row.id,
            entity_type: row.entity_type,
            data: row.data,
        })
        .collect())
}

/// Writes a model's already-resolved answers for one entity straight to `content_entities`, snapshotting first so `POST /api/migration-jobs/{job_id}/undo` can reverse it.
/// Returns `Ok(true)` if the write landed, `Ok(false)` if it was skipped (the entity's data was not a JSON object, or the write failed for a reason specific to this entity: it no longer exists, or the merged data no longer fits the schema).
///
/// Separated from `infer_fill` itself so this half (snapshot, merge, write, and the failure cleanup) is testable without a real or stubbed LLM endpoint: `answers` is exactly what `InferenceClient::propose_fields` would have returned, supplied directly.
///
/// Snapshot must run before update, not after: it snapshots whatever `content_entities` currently holds, so snapshotting after a successful update would record the entity's *new* data as its own "before" image, and `undo_job` would restore it to the state it is already in rather than the state it held before this call.
/// One snapshot per entity, not one per field: if `answers` has three fields, that is one write and one snapshot, since three snapshots of the same entity under one job would make undo restore an intermediate state depending on which row it read last.
pub async fn apply_answers(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity: &OutdatedEntity,
    job_id: Uuid,
    answers: serde_json::Map<String, Value>,
) -> Result<bool, YorishiroError> {
    let mut data = entity.data.clone();
    let Some(object) = data.as_object_mut() else {
        return Ok(false);
    };
    for (field, value) in answers {
        object.insert(field, value);
    }

    yorishiro_core::models::content_entities::snapshot(conn, workspace_id, entity.id, job_id)
        .await?;

    match yorishiro_core::models::content_entities::update(
        conn,
        workspace_id,
        entity.id,
        data,
        None,
    )
    .await
    {
        Ok(_) => Ok(true),
        Err(YorishiroError::NotFound { .. } | YorishiroError::ValidationFailed { .. }) => {
            // The snapshot just taken now describes a change that never landed: the entity's data
            // is unchanged, so undo restoring from it would be a no-op today, but the row still
            // exists under job_id and would falsely attribute a *later*, unrelated edit to this
            // job if that edit happens before an eventual undo. Removing it here keeps job_id's
            // snapshot set limited to entities this call actually changed.
            yorishiro_core::models::content_entities::delete_snapshot(
                conn,
                workspace_id,
                entity.id,
                job_id,
            )
            .await?;
            Ok(false)
        }
        Err(err) => Err(err),
    }
}
