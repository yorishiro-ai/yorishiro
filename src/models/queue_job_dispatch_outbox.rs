use std::fmt;

use sea_orm::entity::prelude::*;
use sea_orm::{
    ActiveValue::Set, Condition, ConnectionTrait, DatabaseConnection, DbErr, ExprTrait, QueryOrder,
    QuerySelect, TransactionTrait,
};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{
    queue_job_dispatch_outbox as outbox, queue_job_dispatch_outbox::Model,
    queue_job_lifecycles as lifecycle,
};
use crate::models::queue_job_lifecycles::LifecycleStatus;
use crate::workers::embedding_sync::WorkerClass;

const RETRY_COOLDOWN_SECONDS: i64 = 60;
const RECOVERY_BATCH_SIZE: u64 = 100;
const QUERY_EMBEDDING_PRIORITY: i32 = 1000;

impl fmt::Debug for Model {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QueueJobDispatchOutbox")
            .field("id", &self.id)
            .field("lifecycle_id", &self.lifecycle_id)
            .field("payload", &self.payload)
            .field("worker_name", &self.worker_name)
            .field("queue_name", &self.queue_name)
            .field("tags", &self.tags)
            .field("last_attempt_at", &self.last_attempt_at)
            .field("deadline_at", &self.deadline_at)
            .field("published_at", &self.published_at)
            .field("claim_token", &"[REDACTED]")
            .field("claim_until", &self.claim_until)
            .field("priority", &self.priority)
            .field("workspace_id", &self.workspace_id)
            .finish()
    }
}

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for outbox::ActiveModel {}

#[derive(Clone)]
pub struct PendingDispatch {
    pub id: Uuid,
    pub lifecycle_id: Option<Uuid>,
    pub(crate) job_name: String,
    pub(crate) worker_class: WorkerClass,
    pub payload: Value,
    pub worker_name: String,
    pub queue_name: Option<String>,
    pub tags: Option<Vec<String>>,
    pub deadline_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub workspace_id: Option<Uuid>,
    pub priority: Option<i32>,
}

fn payload_workspace_id(payload: &Value) -> Option<Uuid> {
    payload
        .get("workspace_id")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
}

fn tags_json(tags: Option<Vec<String>>) -> Result<Option<Value>, DbErr> {
    tags.map(serde_json::to_value)
        .transpose()
        .map_err(|error| DbErr::Custom(error.to_string()))
}

/// Records the lifecycle row and its dispatch payload in one transaction.
///
/// # Errors
/// Returns an error if the payload cannot be serialized or the transaction fails.
pub async fn record(
    db: &DatabaseConnection,
    lifecycle: crate::models::queue_job_lifecycles::Enqueue<'_>,
    payload: Value,
    worker_name: String,
    queue_name: Option<String>,
    tags: Option<Vec<String>>,
) -> Result<(), DbErr> {
    let txn = db.begin().await?;
    record_in_transaction(&txn, lifecycle, payload, worker_name, queue_name, tags).await?;
    txn.commit().await
}

/// Records a lifecycle row and its outbox row in the caller's transaction.
///
/// # Errors
/// Returns an error if the database operation fails.
pub async fn record_in_transaction(
    txn: &impl ConnectionTrait,
    lifecycle: crate::models::queue_job_lifecycles::Enqueue<'_>,
    mut payload: Value,
    worker_name: String,
    queue_name: Option<String>,
    tags: Option<Vec<String>>,
) -> Result<(), DbErr> {
    let id = lifecycle.id;
    crate::models::queue_job_lifecycles::Entity::record_enqueue(txn, lifecycle).await?;
    if let Value::Object(fields) = &mut payload {
        fields.insert("lifecycle_id".into(), Value::String(id.to_string()));
    }
    outbox::ActiveModel {
        id: Set(id),
        lifecycle_id: Set(Some(id)),
        workspace_id: Set(payload_workspace_id(&payload)),
        payload: Set(payload),
        worker_name: Set(worker_name),
        queue_name: Set(queue_name),
        tags: Set(tags_json(tags)?),
        ..Default::default()
    }
    .insert(txn)
    .await?;
    Ok(())
}

/// Records a search-embedding dispatch that expires at `deadline_at` and has no lifecycle row.
///
/// # Errors
/// Returns an error if the database operation fails.
pub async fn record_query_in_transaction(
    txn: &impl ConnectionTrait,
    id: Uuid,
    deadline_at: chrono::DateTime<chrono::FixedOffset>,
    payload: Value,
    worker_name: String,
    queue_name: Option<String>,
    tags: Option<Vec<String>>,
) -> Result<(), DbErr> {
    outbox::ActiveModel {
        id: Set(id),
        workspace_id: Set(payload_workspace_id(&payload)),
        priority: Set(Some(QUERY_EMBEDDING_PRIORITY)),
        payload: Set(payload),
        worker_name: Set(worker_name),
        queue_name: Set(queue_name),
        tags: Set(tags_json(tags)?),
        deadline_at: Set(Some(deadline_at)),
        ..Default::default()
    }
    .insert(txn)
    .await?;
    Ok(())
}

/// Puts a retryable lifecycle back in the queue with a fresh payload.
///
/// # Errors
/// Returns an error if the lifecycle is not queued or retrying, or the database operation fails.
pub async fn record_lifecycle_payload(
    txn: &impl ConnectionTrait,
    lifecycle_id: Uuid,
    mut payload: Value,
    worker_name: String,
    queue_name: Option<String>,
    tags: Option<Vec<String>>,
) -> Result<(), DbErr> {
    if let Value::Object(fields) = &mut payload {
        fields.insert(
            "lifecycle_id".into(),
            Value::String(lifecycle_id.to_string()),
        );
    }
    let lifecycle_update = crate::models::queue_job_lifecycles::Entity::update_many()
        .col_expr(
            lifecycle::Column::Status,
            Expr::value(LifecycleStatus::Queued.as_db_str()),
        )
        .col_expr(lifecycle::Column::Attempt, Expr::value(0))
        .filter(lifecycle::Column::Id.eq(lifecycle_id))
        .filter(lifecycle::Column::Status.is_in([
            LifecycleStatus::Retrying.as_db_str(),
            LifecycleStatus::Queued.as_db_str(),
        ]))
        .exec(txn)
        .await?;
    if lifecycle_update.rows_affected != 1 {
        return Err(DbErr::RecordNotFound("retryable queue lifecycle".into()));
    }
    let result = outbox::Entity::update_many()
        .set(outbox::ActiveModel {
            workspace_id: Set(payload_workspace_id(&payload)),
            payload: Set(payload),
            worker_name: Set(worker_name),
            queue_name: Set(queue_name),
            tags: Set(tags_json(tags)?),
            priority: Set(None),
            last_attempt_at: Set(None),
            published_at: Set(None),
            claim_token: Set(None),
            claim_until: Set(None),
            ..Default::default()
        })
        .filter(outbox::Column::LifecycleId.eq(Some(lifecycle_id)))
        .exec(txn)
        .await?;
    if result.rows_affected == 0 {
        return Err(DbErr::RecordNotFound(
            "lifecycle dispatch outbox row".into(),
        ));
    }
    Ok(())
}

/// Dispatches whose lifecycle is waiting for the queue and whose retry cooldown has passed.
///
/// # Errors
/// Returns an error if the query fails or a stored row cannot be decoded.
pub async fn due(db: &impl ConnectionTrait) -> Result<Vec<PendingDispatch>, YorishiroError> {
    use sea_orm::{
        JoinType,
        sea_query::{Alias, Func, SimpleExpr},
    };

    let sqlite = db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite;
    let retry_at: SimpleExpr = Expr::col((lifecycle::Entity, lifecycle::Column::RetryAt));
    let (retry_at, now, cooldown): (SimpleExpr, SimpleExpr, SimpleExpr) = if sqlite {
        (
            Func::cust(Alias::new("datetime")).arg(retry_at).into(),
            Func::cust(Alias::new("datetime")).arg("now").into(),
            Func::cust(Alias::new("datetime"))
                .args([
                    Expr::val("now"),
                    Expr::val(format!("-{RETRY_COOLDOWN_SECONDS} seconds")),
                ])
                .into(),
        )
    } else {
        (
            retry_at,
            Expr::current_timestamp(),
            Expr::current_timestamp().sub(
                Expr::val(format!("{RETRY_COOLDOWN_SECONDS} seconds"))
                    .cast_as(Alias::new("interval")),
            ),
        )
    };
    let rows = outbox::Entity::find()
        .join(
            JoinType::LeftJoin,
            outbox::Entity::belongs_to(lifecycle::Entity)
                .from(outbox::Column::LifecycleId)
                .to(lifecycle::Column::Id)
                .into(),
        )
        .filter(outbox::Column::PublishedAt.is_null())
        .filter(
            Condition::any()
                .add(
                    Condition::all()
                        .add(lifecycle::Column::Id.is_not_null())
                        .add(
                            Condition::any()
                                .add(
                                    lifecycle::Column::Status
                                        .eq(LifecycleStatus::Queued.as_db_str()),
                                )
                                .add(
                                    Condition::all()
                                        .add(
                                            lifecycle::Column::Status
                                                .eq(LifecycleStatus::Retrying.as_db_str()),
                                        )
                                        .add(
                                            Condition::any()
                                                .add(lifecycle::Column::RetryAt.is_null())
                                                .add(retry_at.lte(now)),
                                        ),
                                ),
                        ),
                )
                .add(
                    Condition::all()
                        .add(lifecycle::Column::Id.is_null())
                        .add(outbox::Column::WorkspaceId.is_not_null())
                        .add(outbox::Column::WorkerName.eq("QueryEmbeddingWorker"))
                        .add(
                            Expr::col((outbox::Entity, outbox::Column::DeadlineAt))
                                .gt(Expr::current_timestamp()),
                        ),
                ),
        )
        .filter(
            Condition::any()
                .add(outbox::Column::LastAttemptAt.is_null())
                .add(Expr::col((outbox::Entity, outbox::Column::LastAttemptAt)).lte(cooldown)),
        )
        .order_by_asc(outbox::Column::Id)
        .limit(RECOVERY_BATCH_SIZE)
        .select_also(lifecycle::Entity)
        .all(db)
        .await
        .internal()?;
    rows.into_iter()
        .map(|(row, lifecycle)| {
            let tags = row
                .tags
                .map(serde_json::from_value)
                .transpose()
                .internal()?;
            Ok(PendingDispatch {
                id: row.id,
                lifecycle_id: row.lifecycle_id,
                job_name: lifecycle
                    .as_ref()
                    .map_or_else(|| "query_embedding".to_owned(), |row| row.job_name.clone()),
                worker_class: lifecycle
                    .as_ref()
                    .and_then(|row| WorkerClass::from_db_str(&row.worker_class))
                    .unwrap_or(WorkerClass::Shared),
                payload: row.payload,
                worker_name: row.worker_name,
                queue_name: row.queue_name,
                tags,
                deadline_at: row.deadline_at,
                workspace_id: row.workspace_id,
                priority: row.priority,
            })
        })
        .collect()
}

/// Takes a lease on an unpublished dispatch so only one relay publishes it.
///
/// # Errors
/// Returns an error if the database operation fails.
pub async fn claim_dispatch(
    db: &DatabaseConnection,
    id: Uuid,
    claim_token: Uuid,
    lease_seconds: i64,
) -> Result<bool, DbErr> {
    let lease_until = crate::time::now() + chrono::Duration::seconds(lease_seconds);
    let result = outbox::Entity::update_many()
        .col_expr(outbox::Column::LastAttemptAt, Expr::current_timestamp())
        .col_expr(
            outbox::Column::ClaimToken,
            Expr::val(claim_token.to_string()),
        )
        .col_expr(outbox::Column::ClaimUntil, Expr::val(lease_until))
        .filter(outbox::Column::Id.eq(id))
        .filter(outbox::Column::PublishedAt.is_null())
        .filter(
            outbox::Column::ClaimUntil
                .is_null()
                .or(Expr::col(outbox::Column::ClaimUntil).lte(Expr::current_timestamp())),
        )
        .exec(db)
        .await?;
    Ok(result.rows_affected == 1)
}

/// Records that the provider accepted a claimed dispatch.
///
/// # Errors
/// Returns an error if the database operation fails.
pub async fn acknowledge_dispatch(
    db: &DatabaseConnection,
    id: Uuid,
    claim_token: Uuid,
    provider_job_id: &str,
) -> Result<bool, DbErr> {
    let txn = db.begin().await?;
    let result = outbox::Entity::update_many()
        .col_expr(outbox::Column::PublishedAt, Expr::current_timestamp())
        .col_expr(outbox::Column::ClaimToken, Expr::val(None::<String>))
        .col_expr(
            outbox::Column::ClaimUntil,
            Expr::val(None::<chrono::DateTime<chrono::FixedOffset>>),
        )
        .filter(outbox::Column::Id.eq(id))
        .filter(outbox::Column::ClaimToken.eq(claim_token.to_string()))
        .filter(Expr::col(outbox::Column::ClaimUntil).gt(Expr::current_timestamp()))
        .filter(outbox::Column::PublishedAt.is_null())
        .exec(&txn)
        .await?;
    if result.rows_affected != 1 {
        txn.rollback().await?;
        return Ok(false);
    }
    if let Some(lifecycle_id) = outbox::Entity::find_by_id(id)
        .one(&txn)
        .await?
        .and_then(|row| row.lifecycle_id)
    {
        crate::models::queue_job_lifecycles::Entity::mark_dispatched(
            &txn,
            lifecycle_id,
            provider_job_id,
        )
        .await?;
    }
    txn.commit().await?;
    Ok(true)
}

/// Drops a dispatch whose deadline passed.
///
/// # Errors
/// Returns an error if the database operation fails.
pub(crate) async fn expire(db: &impl ConnectionTrait, id: Uuid) -> Result<(), DbErr> {
    outbox::Entity::delete_by_id(id).exec(db).await?;
    Ok(())
}
