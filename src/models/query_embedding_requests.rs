//! The durable request and result of one semantic-search query embedding.
//!
//! The API and MCP server holds no embedding model, so it cannot turn query text into a vector itself.
//! It opens a `pending` row here and enqueues a job that names the row.
//! A worker reads the text, embeds it as a query, and completes the row with the vector, or fails it with a diagnostic.
//! The server polls the row and consumes it, which deletes it.
//!
//! Every transition is fenced on the row's current status, so a late worker cannot overwrite a request the server already expired or consumed, and two consumers cannot both take one result.

use chrono::{Duration, Utc};
use sea_orm::ActiveValue::Set;
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::Expr;
use sea_orm::{ConnectionTrait, QueryFilter};

pub use crate::models::_entities::query_embedding_requests::{ActiveModel, Column, Entity, Model};

use crate::db_enum::db_enum;
use crate::error::YorishiroError;

db_enum! {
    /// Where a query embedding request stands.
    /// Matches `status`'s CHECK constraint string-for-string.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum QueryEmbeddingStatus {
        Pending = "pending",
        Succeeded = "succeeded",
        Failed = "failed",
        Expired = "expired",
    }
}

/// What reading a request found.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryOutcome {
    /// No worker has answered yet.
    Pending,
    /// The worker stored a vector.
    /// The row was deleted by the read that returned this.
    Ready { vector: Vec<f32>, model: String },
    /// The worker could not embed the query.
    /// The row was deleted by the read that returned this.
    Failed(String),
    /// The server gave up waiting.
    /// The row was deleted by the read that returned this.
    Expired,
}

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.updated_at = crate::db::stamped_updated_at(insert, this.updated_at);
        Ok(this)
    }
}

impl Entity {
    /// Opens a `pending` request for `query_text` and returns its id.
    /// The row expires `retention_seconds` from now, whether or not anyone reads it.
    ///
    /// # Errors
    /// Returns an error if the row cannot be inserted.
    pub async fn open(
        db: &impl ConnectionTrait,
        workspace_id: Uuid,
        query_text: &str,
        retention_seconds: u64,
    ) -> Result<Uuid, DbErr> {
        let id = Uuid::now_v7();
        let ttl = Duration::seconds(i64::try_from(retention_seconds).unwrap_or(i64::MAX / 1000));
        ActiveModel {
            id: Set(id),
            workspace_id: Set(workspace_id),
            query_text: Set(query_text.to_owned()),
            status: Set(QueryEmbeddingStatus::Pending.as_db_str().to_owned()),
            expires_at: Set((Utc::now() + ttl).fixed_offset()),
            ..Default::default()
        }
        .insert(db)
        .await?;
        Ok(id)
    }

    /// The request `id` of `workspace_id` if it is still waiting for a worker and has not passed its expiry.
    ///
    /// # Errors
    /// Returns an error if the read fails.
    pub async fn find_pending(
        db: &impl ConnectionTrait,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<Option<Model>, DbErr> {
        Entity::find_by_id(id)
            .filter(Column::WorkspaceId.eq(workspace_id))
            .filter(Column::Status.eq(QueryEmbeddingStatus::Pending.as_db_str()))
            .filter(Column::ExpiresAt.gt(Utc::now().fixed_offset()))
            .one(db)
            .await
    }

    /// Stores `vector` for a request that is still `pending`.
    /// Returns `false` when the request was already expired, consumed or purged, so a late result is discarded rather than resurrected.
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub async fn complete(
        db: &impl ConnectionTrait,
        workspace_id: Uuid,
        id: Uuid,
        vector: &[f32],
        model: &str,
    ) -> Result<bool, DbErr> {
        let dimensions = i32::try_from(vector.len())
            .map_err(|_| DbErr::Custom("query vector is too wide to store".into()))?;
        let now = Utc::now().fixed_offset();
        let result = Entity::update_many()
            .col_expr(
                Column::Status,
                Expr::value(QueryEmbeddingStatus::Succeeded.as_db_str()),
            )
            .col_expr(Column::ResultVector, Expr::value(serde_json::json!(vector)))
            .col_expr(Column::Dimensions, Expr::value(dimensions))
            .col_expr(Column::Model, Expr::value(model.to_owned()))
            .col_expr(Column::CompletedAt, Expr::value(now))
            .col_expr(Column::UpdatedAt, Expr::value(now))
            .filter(Column::Id.eq(id))
            .filter(Column::WorkspaceId.eq(workspace_id))
            .filter(Column::Status.eq(QueryEmbeddingStatus::Pending.as_db_str()))
            .exec(db)
            .await?;
        Ok(result.rows_affected == 1)
    }

    /// Records why a `pending` request could not be answered.
    /// Returns `false` when it was no longer pending.
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub async fn fail(
        db: &impl ConnectionTrait,
        workspace_id: Uuid,
        id: Uuid,
        error: &str,
    ) -> Result<bool, DbErr> {
        let now = Utc::now().fixed_offset();
        let result = Entity::update_many()
            .col_expr(
                Column::Status,
                Expr::value(QueryEmbeddingStatus::Failed.as_db_str()),
            )
            .col_expr(Column::Error, Expr::value(error.to_owned()))
            .col_expr(Column::CompletedAt, Expr::value(now))
            .col_expr(Column::UpdatedAt, Expr::value(now))
            .filter(Column::Id.eq(id))
            .filter(Column::WorkspaceId.eq(workspace_id))
            .filter(Column::Status.eq(QueryEmbeddingStatus::Pending.as_db_str()))
            .exec(db)
            .await?;
        Ok(result.rows_affected == 1)
    }

    /// Marks a request nobody answered in time as `expired`.
    /// Returns `false` when a worker finished it first, in which case the caller reads the result instead.
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub async fn expire(
        db: &impl ConnectionTrait,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<bool, DbErr> {
        let result = Entity::update_many()
            .col_expr(
                Column::Status,
                Expr::value(QueryEmbeddingStatus::Expired.as_db_str()),
            )
            .col_expr(Column::UpdatedAt, Expr::value(Utc::now().fixed_offset()))
            .filter(Column::Id.eq(id))
            .filter(Column::WorkspaceId.eq(workspace_id))
            .filter(Column::Status.eq(QueryEmbeddingStatus::Pending.as_db_str()))
            .exec(db)
            .await?;
        Ok(result.rows_affected == 1)
    }

    /// Reads a request, consuming it once it is finished.
    ///
    /// A finished row (succeeded, failed or expired) is deleted by the same statement that returns it, so of two concurrent consumers exactly one gets the outcome and the other gets `None`.
    /// `None` also means the row never existed or was already purged.
    ///
    /// # Errors
    /// Returns `Internal` when the read fails or the stored vector is malformed.
    pub async fn take(
        db: &impl ConnectionTrait,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<Option<QueryOutcome>, YorishiroError> {
        let finished = Entity::delete_many()
            .filter(Column::Id.eq(id))
            .filter(Column::WorkspaceId.eq(workspace_id))
            .filter(Column::Status.ne(QueryEmbeddingStatus::Pending.as_db_str()))
            .exec_with_returning(db)
            .await
            .map_err(|error| YorishiroError::Internal(error.into()))?;
        if let Some(row) = finished.into_iter().next() {
            return Ok(Some(outcome(row)?));
        }
        let waiting = Entity::find_by_id(id)
            .filter(Column::WorkspaceId.eq(workspace_id))
            .one(db)
            .await
            .map_err(|error| YorishiroError::Internal(error.into()))?;
        Ok(waiting.map(|_| QueryOutcome::Pending))
    }

    /// Deletes every request past its expiry, whatever its status, and returns how many.
    ///
    /// # Errors
    /// Returns an error if the delete fails.
    pub async fn purge_expired(db: &impl ConnectionTrait) -> Result<u64, DbErr> {
        let result = Entity::delete_many()
            .filter(Column::ExpiresAt.lt(Utc::now().fixed_offset()))
            .exec(db)
            .await?;
        Ok(result.rows_affected)
    }
}

fn outcome(row: Model) -> Result<QueryOutcome, YorishiroError> {
    let status = QueryEmbeddingStatus::from_db_str(&row.status).ok_or_else(|| {
        YorishiroError::Internal(anyhow::anyhow!(
            "unknown query embedding status {:?}",
            row.status
        ))
    })?;
    match status {
        QueryEmbeddingStatus::Succeeded => {
            let vector = row
                .result_vector
                .and_then(|value| serde_json::from_value::<Vec<f32>>(value).ok())
                .filter(|vector| {
                    row.dimensions
                        .is_some_and(|width| usize::try_from(width) == Ok(vector.len()))
                })
                .ok_or_else(|| {
                    YorishiroError::Internal(anyhow::anyhow!(
                        "the stored query embedding is malformed"
                    ))
                })?;
            Ok(QueryOutcome::Ready {
                vector,
                model: row.model.unwrap_or_default(),
            })
        }
        QueryEmbeddingStatus::Failed => Ok(QueryOutcome::Failed(
            row.error
                .unwrap_or_else(|| "query embedding failed".to_owned()),
        )),
        QueryEmbeddingStatus::Expired => Ok(QueryOutcome::Expired),
        QueryEmbeddingStatus::Pending => Ok(QueryOutcome::Pending),
    }
}
