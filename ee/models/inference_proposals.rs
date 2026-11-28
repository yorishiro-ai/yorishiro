//! Durable, reviewable values produced by infer-fill.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::inference_proposals::Column;
use crate::models::_entities::inference_proposals::{ActiveModel, Entity, Model};
use crate::models::schema_schemas::SchemaStatus;
use crate::models::{entity_entities, schema_schemas};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, SelectExt, Set, SqlErr,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

pub(crate) async fn for_job_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::access_keys::AuthContext,
    job_id: Uuid,
) -> Result<Vec<ProposalRecord>, YorishiroError> {
    let txn = super::iam::access::workspace_transaction(
        db,
        actor,
        crate::models::iam::policy::Action::ListInferenceProposals,
    )
    .await?;
    let workspace_id = actor.require_workspace()?;
    super::inference_jobs::workspace_job(&txn, workspace_id, job_id).await?;
    for_job(&txn, workspace_id, job_id).await
}

pub(crate) async fn discard_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::access_keys::AuthContext,
    job_id: Uuid,
) -> Result<ProposalActionReport, YorishiroError> {
    let txn = super::iam::access::workspace_transaction(
        db,
        actor,
        crate::models::iam::policy::Action::DiscardInferenceProposals,
    )
    .await?;
    let workspace_id = actor.require_workspace()?;
    super::inference_jobs::workspace_job(&txn, workspace_id, job_id).await?;
    let report = discard(&txn, workspace_id, job_id).await?;
    txn.commit().await.internal()?;
    Ok(report)
}

crate::models::db_enum::db_enum! {
    #[derive(Debug, Clone, Copy, Eq, PartialEq)]
    pub enum ProposalStatus {
        Pending = "pending",
        Confirmed = "confirmed",
        Discarded = "discarded",
        Stale = "stale",
        Invalid = "invalid",
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[schemars(rename = "InferenceProposalResponse")]
pub struct ProposalRecord {
    #[serde(rename = "proposal_id")]
    pub id: Uuid,
    #[serde(rename = "inference_job_id")]
    pub job_id: Uuid,
    pub workspace_id: Uuid,
    pub entity_id: Uuid,
    pub schema_id: Uuid,
    pub schema_version: i32,
    pub source_field: String,
    pub proposed: Value,
    pub status: ProposalStatus,
}

impl TryFrom<Model> for ProposalRecord {
    type Error = YorishiroError;

    fn try_from(row: Model) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            job_id: row.job_id,
            workspace_id: row.workspace_id,
            entity_id: row.entity_id,
            schema_id: row.schema_id,
            schema_version: row.schema_version,
            source_field: row.source_field,
            proposed: row.proposed,
            status: row
                .status
                .parse()
                .map_err(|message: String| YorishiroError::Internal((message).into()))?,
        })
    }
}

#[derive(Clone, Serialize, schemars::JsonSchema)]
#[schemars(rename = "ProposalConfirmResponse")]
pub struct ConfirmReport {
    pub job_id: Uuid,
    pub confirmed: i64,
    pub stale: i64,
    pub invalid: i64,
}

#[derive(Clone, Serialize, schemars::JsonSchema)]
#[schemars(rename = "ProposalActionResponse")]
pub struct ProposalActionReport {
    #[serde(rename = "inference_job_id")]
    pub job_id: Uuid,
    pub changed: i64,
}

/// Records proposals only for the currently running attempt.
///
/// # Errors
/// Rejects stale attempts, invalid jobs and persistence failures.
pub async fn record_batch_attempt(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
    attempt: i32,
    schema_id: Uuid,
    schema_version: i32,
    fields: impl IntoIterator<Item = (Uuid, String, Value)>,
) -> Result<i64, YorishiroError> {
    let Some(row) = crate::models::_entities::inference_jobs::Entity::find_by_id(job_id)
        .lock_exclusive()
        .one(conn)
        .await
        .internal()?
    else {
        return Err(YorishiroError::not_found("infer-fill job not found"));
    };
    let job = crate::edition::ee::models::inference_jobs::InferenceJobRecord::try_from(row)?;
    if job.workspace_id != workspace_id {
        return Err(YorishiroError::not_found("infer-fill job not found"));
    }
    if job.status != crate::edition::ee::models::inference_jobs::InferenceJobStatus::Running {
        return Err(YorishiroError::Conflict {
            message: format!("inference job '{job_id}' is not running"),
        });
    }
    if job.attempt != attempt {
        return Err(YorishiroError::Conflict {
            message: format!("inference job '{job_id}' attempt is stale"),
        });
    }
    let mut inserted = 0;
    for (entity_id, source_field, proposed) in fields {
        let result = ActiveModel {
            job_id: Set(job_id),
            workspace_id: Set(workspace_id),
            entity_id: Set(entity_id),
            schema_id: Set(schema_id),
            schema_version: Set(schema_version),
            source_field: Set(source_field),
            proposed: Set(proposed),
            status: Set(ProposalStatus::Pending.as_db_str().to_string()),
            ..Default::default()
        }
        .insert(conn)
        .await;
        match result {
            Ok(_) => inserted += 1,
            Err(err) if matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => {}
            Err(err) => return Err(err).internal(),
        }
    }
    Ok(inserted)
}

/// Lists a job's proposals ordered by entity and field.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn for_job(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<Vec<ProposalRecord>, YorishiroError> {
    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::JobId.eq(job_id))
        .order_by_asc(Column::EntityId)
        .order_by_asc(Column::SourceField)
        .all(conn)
        .await
        .internal()?
        .into_iter()
        .map(TryInto::try_into)
        .collect()
}

pub(crate) async fn require_completed_job(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<(), YorishiroError> {
    let Some(job) = crate::edition::ee::models::inference_jobs::get(conn, job_id).await? else {
        return Err(YorishiroError::not_found("infer-fill job not found"));
    };
    if job.workspace_id != workspace_id {
        return Err(YorishiroError::not_found("infer-fill job not found"));
    }
    if job.status != crate::edition::ee::models::inference_jobs::InferenceJobStatus::Completed {
        return Err(YorishiroError::Conflict {
            message: format!(
                "inference job '{job_id}' is not completed and its proposals cannot be changed"
            ),
        });
    }
    let has_proposals = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::JobId.eq(job_id))
        .exists(conn)
        .await
        .internal()?;
    if !has_proposals {
        return Err(YorishiroError::Conflict {
            message: format!("inference job '{job_id}' did not produce proposals"),
        });
    }
    Ok(())
}

async fn transition_pending(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
    status: ProposalStatus,
) -> Result<i64, YorishiroError> {
    let result = Entity::update_many()
        .col_expr(Column::Status, Expr::value(status.as_db_str()))
        .col_expr(Column::UpdatedAt, Expr::value(crate::data::time::now()))
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::JobId.eq(job_id))
        .filter(Column::Status.eq(ProposalStatus::Pending.as_db_str()))
        .exec(conn)
        .await
        .internal()?;
    i64::try_from(result.rows_affected).internal()
}

/// Discards every pending proposal of a completed job.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn discard(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<ProposalActionReport, YorishiroError> {
    lock_job_for_action(conn, job_id).await?;
    require_completed_job(conn, workspace_id, job_id).await?;
    Ok(ProposalActionReport {
        job_id,
        changed: transition_pending(conn, workspace_id, job_id, ProposalStatus::Discarded).await?,
    })
}

/// Applies a completed job's pending proposals to their entities, marking each entity's proposals confirmed, stale or invalid.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn confirm(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
    updated_by: Option<Uuid>,
) -> Result<ConfirmReport, YorishiroError> {
    lock_job_for_action(conn, job_id).await?;
    require_completed_job(conn, workspace_id, job_id).await?;
    let by_entity = pending_by_entity(conn, workspace_id, job_id).await?;
    let mut confirmed = 0;
    let mut stale = 0;
    let mut invalid = 0;

    for (entity_id, proposals) in by_entity {
        let proposal_count = i64::try_from(proposals.len()).internal()?;
        lock_entity_for_confirmation(conn, workspace_id, entity_id).await?;
        let target = schema_schemas::get_by_id(conn, workspace_id, proposals[0].schema_id).await?;
        let active = schema_schemas::get_active_schema(conn, workspace_id, &target.name).await?;
        let existing = match entity_entities::get(conn, workspace_id, entity_id).await {
            Ok(value) => Some(value),
            Err(YorishiroError::NotFound { .. }) => None,
            Err(err) => return Err(err),
        };
        let existing = existing.filter(|existing| {
            target.status == SchemaStatus::Active
                && active.id == target.id
                && !proposals.iter().any(|proposal| {
                    existing
                        .data
                        .as_object()
                        .is_none_or(|object| object.contains_key(&proposal.source_field))
                })
        });
        let Some(existing) = existing else {
            ensure_entity_marked(
                conn,
                workspace_id,
                job_id,
                entity_id,
                ProposalStatus::Stale.as_db_str(),
                proposals.len(),
            )
            .await?;
            stale += proposal_count;
            continue;
        };

        let Some(data) = confirmed_data(&existing, &target, &proposals)? else {
            ensure_entity_marked(
                conn,
                workspace_id,
                job_id,
                entity_id,
                ProposalStatus::Invalid.as_db_str(),
                proposals.len(),
            )
            .await?;
            invalid += proposal_count;
            continue;
        };

        entity_entities::snapshot(conn, workspace_id, entity_id, job_id).await?;
        if !crate::edition::ee::models::entity_entities::update_if_unchanged(
            conn,
            workspace_id,
            &existing,
            data,
            target.id,
            target.version,
            updated_by,
        )
        .await?
        {
            entity_entities::delete_snapshot(conn, workspace_id, entity_id, job_id).await?;
            ensure_entity_marked(
                conn,
                workspace_id,
                job_id,
                entity_id,
                ProposalStatus::Stale.as_db_str(),
                proposals.len(),
            )
            .await?;
            stale += proposal_count;
            continue;
        }
        ensure_entity_marked(
            conn,
            workspace_id,
            job_id,
            entity_id,
            ProposalStatus::Confirmed.as_db_str(),
            proposals.len(),
        )
        .await?;
        confirmed += proposal_count;
    }

    Ok(ConfirmReport {
        job_id,
        confirmed,
        stale,
        invalid,
    })
}

async fn pending_by_entity(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<BTreeMap<Uuid, Vec<ProposalRecord>>, YorishiroError> {
    let mut by_entity: BTreeMap<Uuid, Vec<ProposalRecord>> = BTreeMap::new();
    for proposal in for_job(conn, workspace_id, job_id)
        .await?
        .into_iter()
        .filter(|row| row.status == ProposalStatus::Pending)
    {
        by_entity
            .entry(proposal.entity_id)
            .or_default()
            .push(proposal);
    }
    if by_entity.is_empty() {
        return Err(YorishiroError::Conflict {
            message: format!("inference job '{job_id}' has no pending proposals"),
        });
    }
    Ok(by_entity)
}

fn confirmed_data(
    existing: &entity_entities::EntityRecord,
    target: &schema_schemas::SchemaRecord,
    proposals: &[ProposalRecord],
) -> Result<Option<Value>, YorishiroError> {
    let (Some(object), Some(type_def)) = (
        existing.data.as_object(),
        target.definition.entity_types.get(&existing.entity_type),
    ) else {
        return Ok(None);
    };
    let mut data = object.clone();
    for proposal in proposals {
        data.insert(proposal.source_field.clone(), proposal.proposed.clone());
    }
    let data = Value::Object(data);
    match entity_entities::validate_data(type_def, &data) {
        Ok(()) => Ok(Some(data)),
        Err(YorishiroError::ValidationFailed { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn lock_job_for_action(
    conn: &impl ConnectionTrait,
    job_id: Uuid,
) -> Result<(), YorishiroError> {
    crate::db::lock_for_update(conn, &format!("inference-proposals:{job_id}"))
        .await
        .internal()
}

async fn lock_entity_for_confirmation(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
) -> Result<(), YorishiroError> {
    entity_entities::Entity::find()
        .select_only()
        .column(crate::models::_entities::entity_entities::Column::Id)
        .filter(crate::models::_entities::entity_entities::Column::WorkspaceId.eq(workspace_id))
        .filter(crate::models::_entities::entity_entities::Column::Id.eq(entity_id))
        .lock_exclusive()
        .into_tuple::<Uuid>()
        .one(conn)
        .await
        .internal()?;
    Ok(())
}

async fn ensure_entity_marked(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
    entity_id: Uuid,
    status: &str,
    expected: usize,
) -> Result<(), YorishiroError> {
    let result = Entity::update_many()
        .col_expr(Column::Status, Expr::value(status))
        .col_expr(Column::UpdatedAt, Expr::value(crate::data::time::now()))
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::JobId.eq(job_id))
        .filter(Column::EntityId.eq(entity_id))
        .filter(Column::Status.eq(ProposalStatus::Pending.as_db_str()))
        .exec(conn)
        .await
        .internal()?;
    if result.rows_affected != u64::try_from(expected).internal()? {
        return Err(YorishiroError::Conflict {
            message: format!("inference job '{job_id}' changed while confirming its proposals"),
        });
    }
    Ok(())
}
