//! Proposed values waiting to be confirmed.
//!
//! Mode A (base's `fill-defaults`) reads a value out of the schema; this one asks a model to guess it.
//! A guess written straight into `content_entities` is indistinguishable afterwards from a value a person entered, so it is held here until a caller confirms the job.
//!
//! Confirming reuses `content_entity_snapshots`: the same `job_id` groups the before-images, so `content_entities::undo_job` reverses a confirmation with no machinery of its own.

use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ConnectionTrait, EntityTrait, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;
use yorishiro_core::error::{ResultExt, YorishiroError};
use yorishiro_core::models::_entities::content_fill_proposals::{ActiveModel, Column, Entity};
use yorishiro_core::models::content_entities;

/// One field's proposed value, as a caller reviews it.
#[derive(Debug, Clone, Serialize, FromQueryResult)]
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
    let active = ActiveModel {
        job_id: ActiveValue::Set(job_id),
        workspace_id: ActiveValue::Set(workspace_id),
        entity_id: ActiveValue::Set(entity_id),
        field_name: ActiveValue::Set(field_name.to_string()),
        proposed: ActiveValue::Set(proposed.clone()),
        ..Default::default()
    };
    Entity::insert(active)
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
    FillProposal::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT entity_id, field_name, proposed FROM content_fill_proposals \
         WHERE workspace_id = $1 AND job_id = $2 \
         ORDER BY entity_id, field_name",
        [workspace_id.into(), job_id.into()],
    ))
    .all(conn)
    .await
    .internal()
}

/// Applies a job's proposals to the entities they were made for.
///
/// Snapshots each entity under the same `job_id` first, so `content_entities::undo_job` reverses the whole confirmation.
/// The proposals are deleted afterwards: leaving them would let the same job be confirmed twice, and the second run would write the same guesses over whatever the first run's undo had restored.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`, which is what gives this its atomicity: a half-applied confirmation would leave the workspace in a state nobody reviewed, and the snapshots would describe a rollback point that never existed.
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
        let Ok(existing) = content_entities::get(conn, workspace_id, entity_id).await else {
            // Deleted between proposal and confirmation.
            skipped += fields.len() as i64;
            continue;
        };

        let mut data = existing.data.clone();
        let Some(object) = data.as_object_mut() else {
            skipped += fields.len() as i64;
            continue;
        };
        for field in &fields {
            object.insert(field.field_name.clone(), field.proposed.clone());
        }

        content_entities::snapshot(conn, workspace_id, entity_id, job_id).await?;

        match content_entities::update(conn, workspace_id, entity_id, data, None).await {
            Ok(_) => applied += fields.len() as i64,
            // The schema rejected a guess.
            // The other entities in this batch were reviewed too, so one bad guess does not discard them.
            Err(_) => skipped += fields.len() as i64,
        }
    }

    conn.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "DELETE FROM content_fill_proposals WHERE workspace_id = $1 AND job_id = $2",
        [workspace_id.into(), job_id.into()],
    ))
    .await
    .internal()?;

    Ok(ConfirmReport {
        job_id,
        applied,
        skipped,
    })
}
