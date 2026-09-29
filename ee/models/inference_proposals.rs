//! Durable, reviewable values produced by infer-fill.

use crate::error::{ResultExt, YorishiroError};
use crate::models::inference_proposals::{ActiveModel, Column, Entity, Model};
use crate::models::schema_schemas::SchemaStatus;
use crate::models::{entity_entities, schema_schemas};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder, Set, SqlErr,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

const PENDING: &str = "pending";
const CONFIRMED: &str = "confirmed";
const REJECTED: &str = "rejected";
const DISCARDED: &str = "discarded";
const STALE: &str = "stale";
const INVALID: &str = "invalid";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    Pending,
    Confirmed,
    Rejected,
    Discarded,
    Stale,
    Invalid,
}

impl ProposalStatus {
    const fn as_db_str(self) -> &'static str {
        match self {
            Self::Pending => PENDING,
            Self::Confirmed => CONFIRMED,
            Self::Rejected => REJECTED,
            Self::Discarded => DISCARDED,
            Self::Stale => STALE,
            Self::Invalid => INVALID,
        }
    }
}

impl fmt::Display for ProposalStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_db_str())
    }
}

impl FromStr for ProposalStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            PENDING => Ok(Self::Pending),
            CONFIRMED => Ok(Self::Confirmed),
            REJECTED => Ok(Self::Rejected),
            DISCARDED => Ok(Self::Discarded),
            STALE => Ok(Self::Stale),
            INVALID => Ok(Self::Invalid),
            _ => Err(format!("unknown inference proposal status: {value}")),
        }
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
            status: Set(PENDING.to_string()),
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
        .filter(Column::Status.eq(PENDING))
        .exec(conn)
        .await
        .internal()?;
    Ok(result.rows_affected as i64)
}

pub async fn reject(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<ProposalActionReport, YorishiroError> {
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
    crate::db::lock_for_update(conn, &format!("inference-proposals:{job_id}"))
        .await
        .internal()?;
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
        let target = schema_schemas::get_by_id(conn, workspace_id, proposals[0].schema_id).await?;
        let active = schema_schemas::get_active_schema(conn, workspace_id, &target.name).await?;
        let existing = match entity_entities::get(conn, workspace_id, entity_id).await {
            Ok(value) => value,
            Err(YorishiroError::NotFound { .. }) => {
                mark_entity(conn, workspace_id, job_id, entity_id, STALE).await?;
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
            mark_entity(conn, workspace_id, job_id, entity_id, STALE).await?;
            stale += proposals.len() as i64;
            continue;
        }

        let Some(object) = existing.data.as_object() else {
            mark_entity(conn, workspace_id, job_id, entity_id, INVALID).await?;
            invalid += proposals.len() as i64;
            continue;
        };
        let mut data = object.clone();
        for proposal in &proposals {
            data.insert(proposal.source_field.clone(), proposal.proposed.clone());
        }
        let Some(type_def) = target.definition.entity_types.get(&existing.entity_type) else {
            mark_entity(conn, workspace_id, job_id, entity_id, INVALID).await?;
            invalid += proposals.len() as i64;
            continue;
        };
        match entity_entities::validate_data(type_def, &Value::Object(data.clone())) {
            Ok(()) => {}
            Err(YorishiroError::ValidationFailed { .. }) => {
                mark_entity(conn, workspace_id, job_id, entity_id, INVALID).await?;
                invalid += proposals.len() as i64;
                continue;
            }
            Err(err) => return Err(err),
        }

        entity_entities::snapshot(conn, workspace_id, entity_id, job_id).await?;
        let active_model = entity_entities::ActiveModel {
            id: ActiveValue::Unchanged(entity_id),
            data: ActiveValue::Set(Value::Object(data)),
            schema_id: ActiveValue::Set(target.id),
            schema_version: ActiveValue::Set(target.version),
            updated_by: ActiveValue::Set(updated_by),
            ..Default::default()
        };
        active_model
            .update_without_returning(conn)
            .await
            .internal()?;
        mark_entity(conn, workspace_id, job_id, entity_id, CONFIRMED).await?;
        confirmed += proposals.len() as i64;
    }

    Ok(ConfirmReport {
        job_id,
        confirmed,
        stale,
        invalid,
    })
}

async fn mark_entity(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
    entity_id: Uuid,
    status: &str,
) -> Result<(), YorishiroError> {
    Entity::update_many()
        .col_expr(Column::Status, Expr::value(status))
        .col_expr(Column::UpdatedAt, Expr::value(chrono::Utc::now()))
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::JobId.eq(job_id))
        .filter(Column::EntityId.eq(entity_id))
        .filter(Column::Status.eq(PENDING))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}
