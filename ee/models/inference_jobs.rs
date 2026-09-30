//! Durable state for asynchronous infer-fill jobs.

use crate::error::{ResultExt, YorishiroError};
use crate::models::inference_jobs::{ActiveModel, Column, Entity, Model};
use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, ExprTrait,
    QueryFilter, QuerySelect, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InferenceJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

impl InferenceJobStatus {
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn from_db_str(value: &str) -> Option<Self> {
        value.parse().ok()
    }
}

impl fmt::Display for InferenceJobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_db_str())
    }
}

impl FromStr for InferenceJobStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("unknown inference job status: {value}")),
        }
    }
}

pub const QUEUED: &str = InferenceJobStatus::Queued.as_db_str();
pub const RUNNING: &str = InferenceJobStatus::Running.as_db_str();
pub const COMPLETED: &str = InferenceJobStatus::Completed.as_db_str();
pub const FAILED: &str = InferenceJobStatus::Failed.as_db_str();

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferenceJobRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub schema_name: String,
    pub status: InferenceJobStatus,
    pub applied: i64,
    pub proposed: i64,
    pub skipped: i64,
    pub error: Option<String>,
    pub(crate) attempt: i32,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    pub updated_at: chrono::DateTime<chrono::FixedOffset>,
}

impl TryFrom<Model> for InferenceJobRecord {
    type Error = YorishiroError;

    fn try_from(row: Model) -> Result<Self, Self::Error> {
        let status = InferenceJobStatus::from_db_str(&row.status).ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!(
                "unknown inference job status: {}",
                row.status
            ))
        })?;
        Ok(Self {
            id: row.id,
            workspace_id: row.workspace_id,
            schema_name: row.schema_name,
            status,
            applied: row.applied,
            proposed: row.proposed,
            skipped: row.skipped,
            error: row.error,
            attempt: row.attempt,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

pub async fn create(
    conn: &impl ConnectionTrait,
    id: Uuid,
    workspace_id: Uuid,
    schema_name: &str,
) -> Result<(), YorishiroError> {
    let active = ActiveModel {
        id: Set(id),
        workspace_id: Set(workspace_id),
        schema_name: Set(schema_name.to_string()),
        status: Set(InferenceJobStatus::Queued.as_db_str().to_string()),
        applied: Set(0),
        skipped: Set(0),
        error: ActiveValue::Set(None),
        attempt: Set(0),
        ..Default::default()
    };
    active.insert(conn).await.internal()?;
    Ok(())
}

pub async fn get(
    conn: &impl ConnectionTrait,
    id: Uuid,
) -> Result<Option<InferenceJobRecord>, YorishiroError> {
    Entity::find_by_id(id)
        .one(conn)
        .await
        .internal()?
        .map(InferenceJobRecord::try_from)
        .transpose()
}

/// Claims a queued job exactly once.
/// A running job is never reclaimed because its worker may still be executing an inference.
pub async fn claim(conn: &impl ConnectionTrait, id: Uuid) -> Result<bool, YorishiroError> {
    claim_attempt(conn, id, None).await
}

pub(crate) async fn claim_attempt(
    conn: &impl ConnectionTrait,
    id: Uuid,
    expected_attempt: Option<i32>,
) -> Result<bool, YorishiroError> {
    let mut update = Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(InferenceJobStatus::Running.as_db_str()),
        )
        .col_expr(Column::Attempt, Expr::col(Column::Attempt).add(1))
        .col_expr(Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(Column::Id.eq(id))
        .filter(Column::Status.eq(InferenceJobStatus::Queued.as_db_str()));
    if let Some(attempt) = expected_attempt {
        update = update.filter(Column::Attempt.eq(attempt));
    }
    let result = update.exec(conn).await.internal()?;
    if result.rows_affected == 1 {
        return Ok(true);
    }

    // A no-op claim may be a missing or stale job, but an invalid persisted status must still
    // follow the model boundary's internal-error path.
    if let Some(row) = Entity::find_by_id(id).one(conn).await.internal()? {
        InferenceJobRecord::try_from(row)?;
    }
    Ok(false)
}

/// Repairs the inference row after lifecycle admission committed before the worker claimed it.
/// The target attempt is the lifecycle attempt, so repeated deliveries and recovery are idempotent.
pub(crate) async fn reconcile_attempt(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
    target_attempt: i32,
) -> Result<bool, YorishiroError> {
    let txn = conn.begin().await.internal()?;
    let Some(row) = Entity::find_by_id(id)
        .lock_exclusive()
        .one(&txn)
        .await
        .internal()?
    else {
        txn.rollback().await.internal()?;
        return Err(YorishiroError::not_found("infer-fill job not found"));
    };
    InferenceJobRecord::try_from(row.clone())?;
    if row.status == InferenceJobStatus::Running.as_db_str() && row.attempt == target_attempt {
        txn.rollback().await.internal()?;
        return Ok(false);
    }
    if row.attempt != target_attempt - 1
        || row.status == InferenceJobStatus::Completed.as_db_str()
        || row.status == InferenceJobStatus::Failed.as_db_str()
    {
        txn.rollback().await.internal()?;
        return Ok(false);
    }
    let result = Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(InferenceJobStatus::Running.as_db_str()),
        )
        .col_expr(Column::Attempt, Expr::value(target_attempt))
        .col_expr(Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(Column::Id.eq(id))
        .filter(Column::Attempt.eq(target_attempt - 1))
        .filter(Column::Status.is_in([
            InferenceJobStatus::Queued.as_db_str(),
            InferenceJobStatus::Running.as_db_str(),
            InferenceJobStatus::Failed.as_db_str(),
        ]))
        .exec(&txn)
        .await
        .internal()?;
    txn.commit().await.internal()?;
    Ok(result.rows_affected == 1)
}

pub async fn complete(
    conn: &impl ConnectionTrait,
    id: Uuid,
    applied: i64,
    skipped: i64,
) -> Result<(), YorishiroError> {
    let result = Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(InferenceJobStatus::Completed.as_db_str()),
        )
        .col_expr(Column::Applied, Expr::value(applied))
        .col_expr(Column::Skipped, Expr::value(skipped))
        .col_expr(Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(Column::Id.eq(id))
        .filter(Column::Status.eq(InferenceJobStatus::Running.as_db_str()))
        .exec(conn)
        .await
        .internal()?;
    if result.rows_affected == 0 {
        validate_existing_status(conn, id).await?;
    }
    Ok(())
}

/// Completes a proposal-producing job without claiming that entity data was applied.
pub(crate) async fn complete_proposals_attempt(
    conn: &impl ConnectionTrait,
    id: Uuid,
    attempt: i32,
    proposed: i64,
    skipped: i64,
) -> Result<bool, YorishiroError> {
    let result = Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(InferenceJobStatus::Completed.as_db_str()),
        )
        .col_expr(Column::Applied, Expr::value(0))
        .col_expr(Column::Proposed, Expr::value(proposed))
        .col_expr(Column::Skipped, Expr::value(skipped))
        .col_expr(Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(Column::Id.eq(id))
        .filter(Column::Status.eq(InferenceJobStatus::Running.as_db_str()))
        .filter(Column::Attempt.eq(attempt))
        .exec(conn)
        .await
        .internal()?;
    if result.rows_affected == 0 {
        validate_existing_status(conn, id).await?;
    }
    Ok(result.rows_affected == 1)
}

pub async fn fail(
    conn: &impl ConnectionTrait,
    id: Uuid,
    message: &str,
) -> Result<(), YorishiroError> {
    let result = Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(InferenceJobStatus::Failed.as_db_str()),
        )
        .col_expr(Column::Error, Expr::value(message))
        .col_expr(Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(Column::Id.eq(id))
        .filter(Column::Status.is_in([
            InferenceJobStatus::Queued.as_db_str(),
            InferenceJobStatus::Running.as_db_str(),
        ]))
        .exec(conn)
        .await
        .internal()?;
    if result.rows_affected == 0 {
        validate_existing_status(conn, id).await?;
    } else if let Some(row) = Entity::find_by_id(id).one(conn).await.internal()? {
        crate::ee::models::inference_proposals::discard_pending_for_job(conn, row.workspace_id, id)
            .await?;
    }
    Ok(())
}

pub(crate) async fn fail_attempt(
    conn: &impl ConnectionTrait,
    id: Uuid,
    attempt: i32,
    message: &str,
) -> Result<bool, YorishiroError> {
    let result = Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(InferenceJobStatus::Failed.as_db_str()),
        )
        .col_expr(Column::Error, Expr::value(message))
        .col_expr(Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(Column::Id.eq(id))
        .filter(Column::Status.eq(InferenceJobStatus::Running.as_db_str()))
        .filter(Column::Attempt.eq(attempt))
        .exec(conn)
        .await
        .internal()?;
    if result.rows_affected == 0 {
        validate_existing_status(conn, id).await?;
    }
    Ok(result.rows_affected == 1)
}

/// Returns a failed job to the claimable queue state for a durable retry.
pub(crate) async fn retry(
    conn: &impl ConnectionTrait,
    id: Uuid,
    message: &str,
    attempt: i32,
) -> Result<bool, YorishiroError> {
    let result = Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(InferenceJobStatus::Queued.as_db_str()),
        )
        .col_expr(Column::Error, Expr::value(message))
        .col_expr(Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(Column::Id.eq(id))
        .filter(Column::Status.eq(InferenceJobStatus::Failed.as_db_str()))
        .filter(Column::Attempt.eq(attempt))
        .exec(conn)
        .await
        .internal()?;
    if result.rows_affected == 0 {
        validate_existing_status(conn, id).await?;
    }
    Ok(result.rows_affected == 1)
}

async fn validate_existing_status(
    conn: &impl ConnectionTrait,
    id: Uuid,
) -> Result<(), YorishiroError> {
    if let Some(row) = Entity::find_by_id(id).one(conn).await.internal()? {
        InferenceJobRecord::try_from(row)?;
    }
    Ok(())
}
