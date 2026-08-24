//! Proposed values waiting to be confirmed.
//!
//! Mode A (base's `fill-defaults`) reads a value out of the schema; this one asks a model to guess it.
//! A guess written straight into `content_entities` is indistinguishable afterwards from a value a person entered, so it is held here until a caller confirms the job.
//!
//! Confirming reuses `content_entity_snapshots`: the same `job_id` groups the before-images, so `content_entities::undo_job` reverses a confirmation with no machinery of its own.

use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;
use yorishiro_core::error::{ResultExt, YorishiroError};
use yorishiro_core::models::_entities::content_entity_snapshots;
use yorishiro_core::models::_entities::content_fill_proposals::{ActiveModel, Column, Entity};
use yorishiro_core::models::content_entities;

/// One field's proposed value, as a caller reviews it.
#[derive(Debug, Clone, Serialize)]
pub struct FillProposal {
    pub entity_id: Uuid,
    pub field_name: String,
    pub proposed: Value,
}

/// What confirming a job did.
#[derive(Debug, Clone, Serialize)]
pub struct ConfirmReport {
    pub job_id: Uuid,
    /// Entities whose data was changed. Undo takes the same `job_id`.
    pub applied: i64,
    /// Proposals whose entity no longer exists, or whose value the schema rejects.
    /// Skipping is not an error: a proposal is a guess, and one guess failing validation should not stop the rest of a reviewed batch from landing.
    pub skipped: i64,
}

/// Records what a model proposed.
/// Replaces any earlier proposal for the same field in the same job, so re-running inference for a job does not leave two answers with no way to choose.
pub async fn record(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
    entity_id: Uuid,
    field_name: &str,
    proposed: &Value,
) -> Result<(), YorishiroError> {
    record_batch(
        conn,
        workspace_id,
        job_id,
        [(entity_id, field_name.to_string(), proposed.clone())],
    )
    .await
}

/// One entity's field and the value proposed for it, as [`record_batch`] takes them.
pub type ProposedField = (Uuid, String, Value);

/// Records every field a job proposed in one statement, instead of one `INSERT` per field.
/// Same replace-on-conflict behavior as [`record`], which now just calls this with one field.
///
/// `fields` must not repeat a `(entity_id, field_name)` pair: Postgres refuses `ON CONFLICT DO
/// UPDATE` when one statement would affect the same row twice ("command cannot affect row a
/// second time"), unlike single-row upserts, where a repeat was harmless. This is safe as called
/// from `infer_fill`: each entity's proposed fields come from one `InferenceClient::propose_fields`
/// call, which returns a `serde_json::Map` keyed by field name, so it cannot itself contain a
/// duplicate field for one entity, and different entities never share an `entity_id`.
pub async fn record_batch(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
    fields: impl IntoIterator<Item = ProposedField>,
) -> Result<(), YorishiroError> {
    let active_models: Vec<ActiveModel> = fields
        .into_iter()
        .map(|(entity_id, field_name, proposed)| ActiveModel {
            job_id: ActiveValue::Set(job_id),
            workspace_id: ActiveValue::Set(workspace_id),
            entity_id: ActiveValue::Set(entity_id),
            field_name: ActiveValue::Set(field_name),
            proposed: ActiveValue::Set(proposed),
            ..Default::default()
        })
        .collect();
    if active_models.is_empty() {
        return Ok(());
    }
    Entity::insert_many(active_models)
        .on_conflict(
            OnConflict::columns([
                Column::WorkspaceId,
                Column::JobId,
                Column::EntityId,
                Column::FieldName,
            ])
            .update_column(Column::Proposed)
            .to_owned(),
        )
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// Everything proposed for one job, for a caller to review before confirming.
pub async fn for_job(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<Vec<FillProposal>, YorishiroError> {
    let rows = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::JobId.eq(job_id))
        .order_by_asc(Column::EntityId)
        .order_by_asc(Column::FieldName)
        .all(conn)
        .await
        .internal()?;

    Ok(rows
        .into_iter()
        .map(|row| FillProposal {
            entity_id: row.entity_id,
            field_name: row.field_name,
            proposed: row.proposed,
        })
        .collect())
}

/// Removes one entity's snapshot from `job_id`'s group.
/// Used only to back out a snapshot `confirm` took for an entity whose update then failed for a
/// reason specific to that proposal: the snapshot no longer describes a real change, and leaving
/// it would let a later, unrelated edit to the same entity be misattributed to this job on undo.
async fn delete_snapshot(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
    job_id: Uuid,
) -> Result<(), YorishiroError> {
    content_entity_snapshots::Entity::delete_many()
        .filter(content_entity_snapshots::Column::WorkspaceId.eq(workspace_id))
        .filter(content_entity_snapshots::Column::EntityId.eq(entity_id))
        .filter(content_entity_snapshots::Column::JobId.eq(job_id))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// Applies a job's proposals to the entities they were made for.
///
/// Snapshots each entity under the same `job_id` first, so `content_entities::undo_job` reverses the whole confirmation.
/// The proposals are deleted afterwards: leaving them would let the same job be confirmed twice, and the second run would write the same guesses over whatever the first run's undo had restored.
///
/// A per-proposal failure (`NotFound`, `ValidationFailed`: this entity was deleted, or this guess doesn't fit the schema) is counted as `skipped` and the loop moves on to the next entity.
/// Any other failure (chiefly `Internal`, a DB/connection error) is not a verdict on the proposal and is returned outright, aborting `confirm` before the trailing `DELETE` is ever reached.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`, which is what gives this its atomicity: a half-applied confirmation would leave the workspace in a state nobody reviewed, and the snapshots would describe a rollback point that never existed.
/// It is also what makes aborting on an infrastructure failure safe rather than merely convenient: every snapshot, update and (unreached) proposal deletion this call has done rolls back together, so a transient failure never leaves proposals silently discarded while the caller is told they were "skipped."
pub async fn confirm(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<ConfirmReport, YorishiroError> {
    let proposals = for_job(conn, workspace_id, job_id).await?;
    if proposals.is_empty() {
        return Err(YorishiroError::not_found(format!(
            "no proposals for job '{job_id}'"
        )));
    }

    let mut applied = 0i64;
    let mut skipped = 0i64;

    // Grouped per entity: one entity with three proposed fields is one write and one snapshot, not three of each.
    // Three snapshots of the same entity under one job would make undo restore an intermediate state depending on which row it read last.
    let mut by_entity: std::collections::BTreeMap<Uuid, Vec<&FillProposal>> = Default::default();
    for proposal in &proposals {
        by_entity
            .entry(proposal.entity_id)
            .or_default()
            .push(proposal);
    }

    for (entity_id, fields) in by_entity {
        // NotFound/ValidationFailed are outcomes about this one proposal (its entity is gone, or
        // its guess doesn't fit the schema) and are counted as `skipped` so one bad guess doesn't
        // discard the rest of a reviewed batch.
        // Anything else (chiefly `Internal`, a DB/connection failure) is infrastructure, not a
        // verdict on the proposal, and must abort `confirm` outright: the whole function runs on
        // one transaction, so returning `Err` here rolls back every snapshot and update this loop
        // has done so far along with it, rather than leaving a job half-applied while the caller
        // is told some of it merely "didn't fit."
        let existing = match content_entities::get(conn, workspace_id, entity_id).await {
            Ok(existing) => existing,
            Err(YorishiroError::NotFound { .. }) => {
                // Deleted between proposal and confirmation.
                skipped += fields.len() as i64;
                continue;
            }
            Err(err) => return Err(err),
        };

        let mut data = existing.data.clone();
        let Some(object) = data.as_object_mut() else {
            skipped += fields.len() as i64;
            continue;
        };
        for field in &fields {
            object.insert(field.field_name.clone(), field.proposed.clone());
        }

        // Snapshot must still run before update, not after: it snapshots whatever
        // content_entities currently holds, so snapshotting after a successful update would
        // record the entity's *new* data as its own "before" image, and undo_job would restore
        // it to the state it is already in rather than the state it held before this job.
        content_entities::snapshot(conn, workspace_id, entity_id, job_id).await?;

        match content_entities::update(conn, workspace_id, entity_id, data, None).await {
            Ok(_) => applied += fields.len() as i64,
            Err(YorishiroError::NotFound { .. } | YorishiroError::ValidationFailed { .. }) => {
                // The snapshot just taken now describes a change that never landed: the entity's
                // data is unchanged, so undo restoring from it would be a no-op today, but the
                // row still exists under `job_id` and would falsely attribute a *later*,
                // unrelated edit to this job if that edit happens before an eventual undo.
                // Removing it here keeps `job_id`'s snapshot set limited to entities this
                // confirmation actually changed.
                delete_snapshot(conn, workspace_id, entity_id, job_id).await?;
                skipped += fields.len() as i64;
            }
            Err(err) => return Err(err),
        }
    }

    Entity::delete_many()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::JobId.eq(job_id))
        .exec(conn)
        .await
        .internal()?;

    Ok(ConfirmReport {
        job_id,
        applied,
        skipped,
    })
}
