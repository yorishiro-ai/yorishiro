use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, QueryOrder};
use uuid::Uuid;

use super::{ActiveModel, EntitySnapshot, UndoReport};
use crate::error::{ResultExt, YorishiroError};

/// Records what `entity_id` holds now, tagged with `job_id`.
///
/// One statement (`INSERT ... SELECT`), not a read followed by a write: under READ COMMITTED, two statements can see different committed data, so a separate read could snapshot an image the row no longer holds by the time the insert runs.
///
/// `entity_snapshots`'s RLS policy matches nothing rather than raising when no workspace is named, so a wrong `workspace_id` or missing entity silently inserts zero rows: `rows_affected == 0` is the only signal that catches it.
pub async fn snapshot(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
    job_id: Uuid,
) -> Result<(), YorishiroError> {
    let rows_affected = super::backend::insert_snapshot(conn, workspace_id, entity_id, job_id)
        .await
        .internal()?;

    if rows_affected == 0 {
        return Err(YorishiroError::not_found(format!(
            "entity '{entity_id}' was not found"
        )));
    }
    Ok(())
}

/// Removes one entity's snapshot from `job_id`'s group.
///
/// For a caller that takes a snapshot before a write it isn't certain will land (`ee/`'s `infer_fill`, writing a model's guess straight to the entity): if that write then fails for a reason specific to it, the snapshot no longer describes a real change, and leaving it would let a later, unrelated edit to the same entity be misattributed to this job on undo.
pub async fn delete_snapshot(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
    job_id: Uuid,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::entity_snapshots;
    entity_snapshots::Entity::delete_many()
        .filter(entity_snapshots::Column::WorkspaceId.eq(workspace_id))
        .filter(entity_snapshots::Column::EntityId.eq(entity_id))
        .filter(entity_snapshots::Column::JobId.eq(job_id))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// Puts every entity in `job_id` back to what it held before.
///
/// An entity deleted since the snapshot is counted rather than failed: refusing the whole undo because one row is gone would leave the rest wrong.
///
/// Restores `schema_id` and `schema_version` alongside `data`, not just `data`: otherwise the entity would claim a version its restored data no longer matches.
/// Builds the `ActiveModel` directly rather than calling `update()`, which cannot set those two columns and would also re-validate and re-stamp `updated_by` for a restore that isn't a user edit.
pub async fn undo_job(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<UndoReport, YorishiroError> {
    use crate::models::_entities::entity_snapshots;

    let snapshots: Vec<EntitySnapshot> = entity_snapshots::Entity::find()
        .filter(entity_snapshots::Column::WorkspaceId.eq(workspace_id))
        .filter(entity_snapshots::Column::JobId.eq(job_id))
        // `id` as a tiebreaker: `created_at` alone is ambiguous when two snapshots land in the same tick.
        // Both are uuidv7 / time-ordered, so this is a second sort key.
        .order_by_asc(entity_snapshots::Column::CreatedAt)
        .order_by_asc(entity_snapshots::Column::Id)
        .into_model::<EntitySnapshot>()
        .all(conn)
        .await
        .internal()?;

    if snapshots.is_empty() {
        return Err(YorishiroError::not_found(format!(
            "no snapshots for job '{job_id}'"
        )));
    }

    let mut restored = 0i64;
    let mut missing = 0i64;

    for snap in &snapshots {
        // `snap.entity_id` is already workspace-scoped by the query above, so no extra filter is needed on the write.
        let active = ActiveModel {
            id: ActiveValue::Unchanged(snap.entity_id),
            data: ActiveValue::Set(snap.data.clone()),
            schema_id: ActiveValue::Set(snap.schema_id),
            schema_version: ActiveValue::Set(snap.schema_version),
            ..Default::default()
        };
        // `update_without_returning` rather than `active.update`: it still calls
        // `before_save` (unlike the raw `Entity::update(...)` builder) and raises
        // `DbErr::RecordNotUpdated` on no match, but decodes no `Model` on return
        // — the `Ok`/`RecordNotUpdated` outcome is all the caller needs.
        let result = active.update_without_returning(conn).await.map(|_| ());
        match result {
            Ok(()) => restored += 1,
            Err(DbErr::RecordNotUpdated) => missing += 1,
            Err(err) => return Err(err).internal(),
        }
    }

    Ok(UndoReport {
        job_id,
        restored,
        missing,
    })
}
