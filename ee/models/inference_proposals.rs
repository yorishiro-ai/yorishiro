//! Durable, reviewable values produced by infer-fill.

use crate::error::{ResultExt, YorishiroError};
use crate::models::inference_proposals::{ActiveModel, Column, Entity, Model};
use crate::models::schema_schemas::SchemaStatus;
use crate::models::{entity_entities, schema_schemas};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, SqlErr,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

crate::db_enum::db_enum! {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ProposalStatus {
        Pending = "pending",
        Confirmed = "confirmed",
        Rejected = "rejected",
        Discarded = "discarded",
        Stale = "stale",
        Invalid = "invalid",
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProposalRecord {
    pub id: Uuid,
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
                .map_err(|message: String| YorishiroError::Internal(anyhow::anyhow!(message)))?,
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ConfirmReport {
    pub job_id: Uuid,
    pub confirmed: i64,
    pub stale: i64,
    pub invalid: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProposalActionReport {
    pub job_id: Uuid,
    pub changed: i64,
}

pub async fn record_batch(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
    schema_id: Uuid,
    schema_version: i32,
    fields: impl IntoIterator<Item = (Uuid, String, Value)>,
) -> Result<i64, YorishiroError> {
    record_batch_attempt(
        conn,
        workspace_id,
        job_id,
        None,
        schema_id,
        schema_version,
        fields,
    )
    .await
}

pub(crate) async fn record_batch_attempt(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
    attempt: Option<i32>,
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
    let job = crate::ee::models::inference_jobs::InferenceJobRecord::try_from(row)?;
    if job.workspace_id != workspace_id {
        return Err(YorishiroError::not_found("infer-fill job not found"));
    }
    if job.status != crate::ee::models::inference_jobs::InferenceJobStatus::Running {
        return Err(YorishiroError::Conflict {
            message: format!("inference job '{job_id}' is not running"),
        });
    }
    if let Some(attempt) = attempt
        && job.attempt != attempt
    {
        return Err(YorishiroError::Conflict {
            message: format!("inference job '{job_id}' attempt is stale"),
        });
    }
    let mut inserted = 0;
    for (entity_id, source_field, proposed) in fields {
        let result = ActiveModel {
            id: Set(Uuid::now_v7()),
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

async fn require_completed_job(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<(), YorishiroError> {
    let Some(job) = crate::ee::models::inference_jobs::get(conn, job_id).await? else {
        return Err(YorishiroError::not_found("infer-fill job not found"));
    };
    if job.workspace_id != workspace_id {
        return Err(YorishiroError::not_found("infer-fill job not found"));
    }
    if job.status != crate::ee::models::inference_jobs::InferenceJobStatus::Completed {
        return Err(YorishiroError::Conflict {
            message: format!(
                "inference job '{job_id}' is not completed and its proposals cannot be changed"
            ),
        });
    }
    let has_proposals = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::JobId.eq(job_id))
        .one(conn)
        .await
        .internal()?
        .is_some();
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
        .col_expr(Column::UpdatedAt, Expr::value(chrono::Utc::now()))
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::JobId.eq(job_id))
        .filter(Column::Status.eq(ProposalStatus::Pending.as_db_str()))
        .exec(conn)
        .await
        .internal()?;
    Ok(result.rows_affected as i64)
}

/// Closes pending proposals when their parent job fails without deleting their audit history.
pub(crate) async fn discard_pending_for_job(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<i64, YorishiroError> {
    transition_pending(conn, workspace_id, job_id, ProposalStatus::Discarded).await
}

pub async fn reject(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<ProposalActionReport, YorishiroError> {
    lock_job_for_action(conn, job_id).await?;
    require_completed_job(conn, workspace_id, job_id).await?;
    Ok(ProposalActionReport {
        job_id,
        changed: transition_pending(conn, workspace_id, job_id, ProposalStatus::Rejected).await?,
    })
}

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

pub async fn confirm(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
    updated_by: Option<Uuid>,
) -> Result<ConfirmReport, YorishiroError> {
    lock_job_for_action(conn, job_id).await?;
    require_completed_job(conn, workspace_id, job_id).await?;
    let pending: Vec<ProposalRecord> = for_job(conn, workspace_id, job_id)
        .await?
        .into_iter()
        .filter(|row| row.status == ProposalStatus::Pending)
        .collect();
    if pending.is_empty() {
        return Err(YorishiroError::Conflict {
            message: format!("inference job '{job_id}' has no pending proposals"),
        });
    }

    let mut by_entity: BTreeMap<Uuid, Vec<ProposalRecord>> = BTreeMap::new();
    for proposal in pending {
        by_entity
            .entry(proposal.entity_id)
            .or_default()
            .push(proposal);
    }
    let mut confirmed = 0;
    let mut stale = 0;
    let mut invalid = 0;

    for (entity_id, proposals) in by_entity {
        lock_entity_for_confirmation(conn, workspace_id, entity_id).await?;
        let target = schema_schemas::get_by_id(conn, workspace_id, proposals[0].schema_id).await?;
        let active = schema_schemas::get_active_schema(conn, workspace_id, &target.name).await?;
        let existing = match entity_entities::get(conn, workspace_id, entity_id).await {
            Ok(value) => value,
            Err(YorishiroError::NotFound { .. }) => {
                ensure_entity_marked(
                    conn,
                    workspace_id,
                    job_id,
                    entity_id,
                    ProposalStatus::Stale.as_db_str(),
                    proposals.len(),
                )
                .await?;
                stale += proposals.len() as i64;
                continue;
            }
            Err(err) => return Err(err),
        };
        let stale_entity = target.status != SchemaStatus::Active
            || active.id != target.id
            || proposals.iter().any(|proposal| {
                existing
                    .data
                    .as_object()
                    .is_none_or(|object| object.contains_key(&proposal.source_field))
            });
        if stale_entity {
            ensure_entity_marked(
                conn,
                workspace_id,
                job_id,
                entity_id,
                ProposalStatus::Stale.as_db_str(),
                proposals.len(),
            )
            .await?;
            stale += proposals.len() as i64;
            continue;
        }

        let Some(object) = existing.data.as_object() else {
            ensure_entity_marked(
                conn,
                workspace_id,
                job_id,
                entity_id,
                ProposalStatus::Invalid.as_db_str(),
                proposals.len(),
            )
            .await?;
            invalid += proposals.len() as i64;
            continue;
        };
        let mut data = object.clone();
        for proposal in &proposals {
            data.insert(proposal.source_field.clone(), proposal.proposed.clone());
        }
        let Some(type_def) = target.definition.entity_types.get(&existing.entity_type) else {
            ensure_entity_marked(
                conn,
                workspace_id,
                job_id,
                entity_id,
                ProposalStatus::Invalid.as_db_str(),
                proposals.len(),
            )
            .await?;
            invalid += proposals.len() as i64;
            continue;
        };
        match entity_entities::validate_data(type_def, &Value::Object(data.clone())) {
            Ok(()) => {}
            Err(YorishiroError::ValidationFailed { .. }) => {
                ensure_entity_marked(
                    conn,
                    workspace_id,
                    job_id,
                    entity_id,
                    ProposalStatus::Invalid.as_db_str(),
                    proposals.len(),
                )
                .await?;
                invalid += proposals.len() as i64;
                continue;
            }
            Err(err) => return Err(err),
        }

        entity_entities::snapshot(conn, workspace_id, entity_id, job_id).await?;
        if !crate::ee::models::entity_entities::update_if_unchanged(
            conn,
            workspace_id,
            &existing,
            Value::Object(data),
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
            stale += proposals.len() as i64;
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
        confirmed += proposals.len() as i64;
    }

    Ok(ConfirmReport {
        job_id,
        confirmed,
        stale,
        invalid,
    })
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
        .col_expr(Column::UpdatedAt, Expr::value(chrono::Utc::now()))
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::JobId.eq(job_id))
        .filter(Column::EntityId.eq(entity_id))
        .filter(Column::Status.eq(ProposalStatus::Pending.as_db_str()))
        .exec(conn)
        .await
        .internal()?;
    if result.rows_affected as usize != expected {
        return Err(YorishiroError::Conflict {
            message: format!("inference job '{job_id}' changed while confirming its proposals"),
        });
    }
    Ok(())
}
