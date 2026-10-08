use sea_orm::entity::prelude::*;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Statement, TransactionTrait};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::workers::embedding_sync::WorkerClass;

const RETRY_COOLDOWN_SECONDS: i64 = 60;
const RECOVERY_BATCH_SIZE: usize = 100;

#[async_trait::async_trait]
impl ActiveModelBehavior for crate::models::_entities::queue_job_dispatch_outbox::ActiveModel {}

#[derive(Clone)]
pub struct PendingDispatch {
    pub lifecycle_id: Uuid,
    pub job_name: String,
    pub worker_class: WorkerClass,
    pub payload: Value,
    pub worker_name: String,
    pub queue_name: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Records the lifecycle row and its dispatch payload in one transaction.
///
/// # Errors
/// Returns an error if the payload cannot be serialized or the transaction fails.
pub async fn record(
    db: &DatabaseConnection,
    lifecycle: crate::models::queue_job_lifecycles::Enqueue<'_>,
    mut payload: Value,
    worker_name: String,
    queue_name: Option<String>,
    tags: Option<Vec<String>>,
) -> Result<(), DbErr> {
    let lifecycle_id = lifecycle.id;
    if let Value::Object(fields) = &mut payload {
        fields.insert(
            "lifecycle_id".into(),
            Value::String(lifecycle_id.to_string()),
        );
    }
    let tags = tags
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| DbErr::Custom(error.to_string()))?;
    let txn = db.begin().await?;
    crate::models::queue_job_lifecycles::Entity::record_enqueue(&txn, lifecycle).await?;
    txn.execute_raw(Statement::from_sql_and_values(
        txn.get_database_backend(),
        "INSERT INTO queue_job_dispatch_outbox (lifecycle_id, payload, worker_name, queue_name, tags) VALUES ($1, $2, $3, $4, $5)",
            [lifecycle_id.into(), payload.into(), worker_name.into(), queue_name.into(), tags.into()],
    ))
    .await?;
    txn.commit().await
}

/// Dispatches whose lifecycle is waiting for the queue and whose retry cooldown has passed.
///
/// # Errors
/// Returns an error if the query fails or a stored row cannot be decoded.
pub async fn due(db: &impl ConnectionTrait) -> Result<Vec<PendingDispatch>, YorishiroError> {
    let backend = db.get_database_backend();
    let cooldown = match backend {
        sea_orm::DatabaseBackend::Sqlite => format!(
            "(o.last_attempt_at IS NULL OR o.last_attempt_at <= datetime('now', '-{RETRY_COOLDOWN_SECONDS} seconds'))"
        ),
        _ => format!(
            "(o.last_attempt_at IS NULL OR o.last_attempt_at <= CURRENT_TIMESTAMP - INTERVAL '{RETRY_COOLDOWN_SECONDS} seconds')"
        ),
    };
    let retry_due = match backend {
        sea_orm::DatabaseBackend::Sqlite => {
            "(l.retry_at IS NULL OR datetime(l.retry_at) <= CURRENT_TIMESTAMP)"
        }
        _ => "(l.retry_at IS NULL OR l.retry_at <= CURRENT_TIMESTAMP)",
    };
    db.query_all_raw(Statement::from_string(
        backend,
        format!(
            "SELECT l.id, l.job_name, l.worker_class, o.payload, o.worker_name, o.queue_name, o.tags \
         FROM queue_job_lifecycles l JOIN queue_job_dispatch_outbox o ON o.lifecycle_id = l.id \
         WHERE ((l.status = 'queued' AND l.provider_job_id IS NULL) OR l.status = 'retrying') \
           AND {retry_due} \
           AND {cooldown} \
          ORDER BY l.enqueue_at LIMIT {RECOVERY_BATCH_SIZE}"
        ),
    ))
    .await
    .internal()?
    .into_iter()
    .map(|row| {
        let payload = if backend == sea_orm::DatabaseBackend::Sqlite {
            serde_json::from_str::<Value>(&row.try_get::<String>("", "payload").internal()?)
                .internal()?
        } else {
            row.try_get("", "payload").internal()?
        };
        let tags = if backend == sea_orm::DatabaseBackend::Sqlite {
            row.try_get::<Option<String>>("", "tags")
                .internal()?
                .map(|json| serde_json::from_str(&json).internal())
                .transpose()?
        } else {
            row.try_get("", "tags").internal()?
        };
        let worker_class_str = row.try_get::<String>("", "worker_class").internal()?;
        let worker_class = WorkerClass::from_db_str(&worker_class_str)
            .ok_or_else(|| DbErr::Type(format!("unknown worker class {worker_class_str:?}")))
            .internal()?;
        Ok(PendingDispatch {
            lifecycle_id: row.try_get("", "id").internal()?,
            job_name: row.try_get("", "job_name").internal()?,
            worker_class,
            payload,
            worker_name: row.try_get("", "worker_name").internal()?,
            queue_name: row.try_get("", "queue_name").internal()?,
            tags,
        })
    })
    .collect()
}

pub(crate) async fn attempted(db: &impl ConnectionTrait, id: Uuid) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "UPDATE queue_job_dispatch_outbox SET last_attempt_at = CURRENT_TIMESTAMP WHERE lifecycle_id = $1",
        [id.into()],
    ))
    .await
    .map(|_| ())
}

/// Stamps the dispatch as attempted when its cooldown has passed, so only one caller recovers it.
///
/// # Errors
/// Returns an error if the update fails.
pub async fn claim_due(db: &impl ConnectionTrait, id: Uuid) -> Result<bool, DbErr> {
    let backend = db.get_database_backend();
    let cooldown = if backend == sea_orm::DatabaseBackend::Sqlite {
        format!(
            "last_attempt_at IS NULL OR last_attempt_at <= datetime('now', '-{RETRY_COOLDOWN_SECONDS} seconds')"
        )
    } else {
        format!(
            "last_attempt_at IS NULL OR last_attempt_at <= CURRENT_TIMESTAMP - INTERVAL '{RETRY_COOLDOWN_SECONDS} seconds'"
        )
    };
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            backend,
            format!("UPDATE queue_job_dispatch_outbox SET last_attempt_at = CURRENT_TIMESTAMP WHERE lifecycle_id = $1 AND ({cooldown})"),
            [id.into()],
        ))
        .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn remove(db: &impl ConnectionTrait, id: Uuid) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "DELETE FROM queue_job_dispatch_outbox WHERE lifecycle_id = $1",
        [id.into()],
    ))
    .await
    .map(|_| ())
}
