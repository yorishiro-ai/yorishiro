//! Proposed values waiting to be confirmed (§FR-8-2 mode B).
//!
//! Mode A reads a value out of the schema; this one asks a model to guess it. A guess written
//! straight into `content.entities` is indistinguishable afterwards from a value a person
//! entered, so it is held here until a caller confirms the job.
//!
//! Confirming reuses `content.entity_snapshots`: the same `job_id` groups the before-images, so
//! `entities::undo_job` reverses a confirmation with no machinery of its own.

use sea_query::{Alias, Expr, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde::Serialize;
use serde_json::Value;
use sqlx::{Connection, PgConnection};
use utoipa::ToSchema;
use uuid::Uuid;

use yorishiro_core::repositories::entities;
use yorishiro_core::{ResultExt, YorishiroError};

#[derive(Iden)]
enum FillProposals {
    Table,
    JobId,
    WorkspaceId,
    EntityId,
    FieldName,
    Proposed,
}

/// One field's proposed value, as a caller reviews it.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
pub struct FillProposal {
    pub entity_id: Uuid,
    pub field_name: String,
    #[schema(value_type = Object)]
    pub proposed: Value,
}

/// What confirming a job did.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConfirmReport {
    pub job_id: Uuid,
    /// Entities whose data was changed. Undo takes the same `job_id`.
    pub applied: i64,
    /// Proposals whose entity no longer exists, or whose value the schema rejects. Skipping is
    /// not an error: a proposal is a guess, and one guess failing validation should not stop
    /// the rest of a reviewed batch from landing.
    pub skipped: i64,
}

/// Records what a model proposed. Replaces any earlier proposal for the same field in the same
/// job, so re-running inference for a job does not leave two answers with no way to choose.
pub async fn record(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    job_id: Uuid,
    entity_id: Uuid,
    field_name: &str,
    proposed: &Value,
) -> Result<(), YorishiroError> {
    let (sql, values) = Query::insert()
        .into_table((Alias::new("content"), FillProposals::Table))
        .columns([
            FillProposals::JobId,
            FillProposals::WorkspaceId,
            FillProposals::EntityId,
            FillProposals::FieldName,
            FillProposals::Proposed,
        ])
        .values_panic([
            job_id.into(),
            workspace_id.into(),
            entity_id.into(),
            field_name.into(),
            proposed.clone().into(),
        ])
        .on_conflict(
            sea_query::OnConflict::columns([
                FillProposals::WorkspaceId,
                FillProposals::JobId,
                FillProposals::EntityId,
                FillProposals::FieldName,
            ])
            .update_column(FillProposals::Proposed)
            .to_owned(),
        )
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_with(&sql, values)
        .execute(&mut *conn)
        .await
        .internal()?;
    Ok(())
}

/// Everything proposed for one job, for a caller to review before confirming.
pub async fn for_job(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<Vec<FillProposal>, YorishiroError> {
    sqlx::query_as::<_, FillProposal>(
        "SELECT entity_id, field_name, proposed \
         FROM content.fill_proposals \
         WHERE workspace_id = $1 AND job_id = $2 \
         ORDER BY entity_id, field_name",
    )
    .bind(workspace_id)
    .bind(job_id)
    .fetch_all(&mut *conn)
    .await
    .internal()
}

/// Applies a job's proposals to the entities they were made for.
///
/// Snapshots each entity under the same `job_id` first, so `entities::undo_job` reverses the
/// whole confirmation. The proposals are deleted afterwards: leaving them would let the same
/// job be confirmed twice, and the second run would write the same guesses over whatever the
/// first run's undo had restored.
///
/// One transaction. A half-applied confirmation would leave the workspace in a state nobody
/// reviewed, and the snapshots would describe a rollback point that never existed.
pub async fn confirm(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<ConfirmReport, YorishiroError> {
    let proposals = for_job(&mut *conn, workspace_id, job_id).await?;
    if proposals.is_empty() {
        return Err(YorishiroError::not_found(format!(
            "no proposals for job {job_id}"
        )));
    }

    let mut applied = 0i64;
    let mut skipped = 0i64;

    let mut tx = conn.begin().await.internal()?;

    // Grouped per entity: one entity with three proposed fields is one write and one snapshot,
    // not three of each. Three snapshots of the same entity under one job would make undo
    // restore an intermediate state depending on which row it read last.
    let mut by_entity: std::collections::BTreeMap<Uuid, Vec<&FillProposal>> = Default::default();
    for proposal in &proposals {
        by_entity
            .entry(proposal.entity_id)
            .or_default()
            .push(proposal);
    }

    for (entity_id, fields) in by_entity {
        let Ok(existing) = entities::get(&mut tx, workspace_id, entity_id).await else {
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

        entities::snapshot(&mut tx, workspace_id, entity_id, job_id).await?;

        match entities::update(&mut tx, workspace_id, entity_id, data, None).await {
            Ok(_) => applied += fields.len() as i64,
            // The schema rejected a guess. The other entities in this batch were reviewed too,
            // so one bad guess does not discard them.
            Err(_) => skipped += fields.len() as i64,
        }
    }

    let (sql, values) = Query::delete()
        .from_table((Alias::new("content"), FillProposals::Table))
        .and_where(Expr::col(FillProposals::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(FillProposals::JobId).eq(job_id))
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values)
        .execute(&mut *tx)
        .await
        .internal()?;

    tx.commit().await.internal()?;

    Ok(ConfirmReport {
        job_id,
        applied,
        skipped,
    })
}

#[cfg(test)]
#[path = "../../tests/services/fill_proposals.rs"]
mod tests;
