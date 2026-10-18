use loco_rs::bgworker::BackgroundWorker;
use std::fmt;

use sea_orm::entity::prelude::*;
use sea_orm::{
    ActiveValue::Set, ConnectionTrait, DatabaseConnection, DbErr, ExprTrait, IntoActiveModel,
    Statement, TransactionTrait,
};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::queue_job_dispatch_outbox::Model;
use crate::workers::embedding_sync::WorkerClass;

const RETRY_COOLDOWN_SECONDS: i64 = 60;
const RECOVERY_BATCH_SIZE: usize = 100;
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
            .field("publishing_epoch", &self.publishing_epoch)
            .field("deadline_at", &self.deadline_at)
            .field("published_at", &self.published_at)
            .field("claim_token", &"[REDACTED]")
            .field("claim_until", &self.claim_until)
            .field("priority", &self.priority)
            .field("workspace_id", &self.workspace_id)
            .field("legacy_quarantined", &self.legacy_quarantined)
            .finish()
    }
}

struct DispatchRow {
    id: Uuid,
    lifecycle_id: Option<Uuid>,
    payload: Value,
    worker_name: String,
    queue_name: Option<String>,
    tags: Option<Vec<String>>,
    publishing_epoch: i64,
}

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for crate::models::_entities::queue_job_dispatch_outbox::ActiveModel {}

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
    pub publishing_epoch: i64,
    pub deadline_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub workspace_id: Option<Uuid>,
    pub priority: Option<i32>,
}

/// Persists the currently active publisher epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishingState {
    pub epoch: i64,
    pub active: bool,
    pub(crate) cutoff_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub snapshot_id: Option<String>,
}

///
/// # Errors
/// Returns an error if the database operation fails.
pub(crate) async fn lock_publisher_shared(txn: &impl ConnectionTrait) -> Result<i64, DbErr> {
    let backend = txn.get_database_backend();
    if backend == sea_orm::DatabaseBackend::Sqlite {
        reserve_publisher_write(txn).await
    } else {
        let row = txn
            .query_one_raw(Statement::from_string(
                txn.get_database_backend(),
                "SELECT public.queue_publishing_epoch_shared_lock() AS epoch",
            ))
            .await?
            .ok_or_else(|| DbErr::RecordNotFound("queue publishing state".into()))?;
        row.try_get("", "epoch")
    }
}

///
/// # Errors
/// Returns an error if the database operation fails.
pub(crate) async fn lock_publisher_exclusive(txn: &impl ConnectionTrait) -> Result<(), DbErr> {
    let backend = txn.get_database_backend();
    if backend == sea_orm::DatabaseBackend::Sqlite {
        reserve_publisher_write(txn).await?;
        return Ok(());
    }
    let row = txn
        .query_one_raw(Statement::from_string(
            txn.get_database_backend(),
            if backend == sea_orm::DatabaseBackend::Postgres {
                "SELECT public.queue_publishing_epoch_exclusive_lock() AS epoch"
            } else {
                "SELECT epoch FROM queue_publishing_state WHERE id = 1"
            },
        ))
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("queue publishing state".into()))?;
    let _: i64 = row.try_get("", "epoch")?;
    Ok(())
}

/// SQLite has no row locks; a no-op write reserves its writer before reading the epoch.
async fn reserve_publisher_write(txn: &impl ConnectionTrait) -> Result<i64, DbErr> {
    use crate::models::_entities::queue_publishing_state::{Column, Entity};
    Entity::update_many()
        .col_expr(Column::Epoch, Expr::col(Column::Epoch))
        .filter(Column::Id.eq(1))
        .exec(txn)
        .await?;
    Entity::find_by_id(1)
        .one(txn)
        .await?
        .map(|row| row.epoch)
        .ok_or_else(|| DbErr::RecordNotFound("queue publishing state".into()))
}

///
/// # Errors
/// Returns an error if the database operation fails.
pub async fn delivery_epoch_allowed(
    db: &impl ConnectionTrait,
    lifecycle_id: Option<Uuid>,
    dispatch_id: Option<Uuid>,
    epoch: Option<i64>,
) -> Result<bool, DbErr> {
    let (Some(dispatch_id), Some(epoch)) = (dispatch_id, epoch) else {
        return Ok(false);
    };
    use crate::models::_entities::queue_job_dispatch_outbox::{Column, Entity};
    let mut query = Entity::find_by_id(dispatch_id);
    if let Some(lifecycle_id) = lifecycle_id {
        query = query.filter(Column::LifecycleId.eq(Some(lifecycle_id)));
    } else {
        query = query.filter(Column::LifecycleId.is_null());
    }
    let stored = query.one(db).await?;
    let current = publishing_state(db).await?;
    Ok(current.active
        && current.epoch == epoch
        && stored.is_some_and(|row| {
            row.publishing_epoch == epoch && (lifecycle_id.is_some() || !row.legacy_quarantined)
        }))
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
    let txn = db.begin().await?;
    record_in_transaction(&txn, lifecycle, payload, worker_name, queue_name, tags).await?;
    txn.commit().await
}

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
    let epoch = lock_publisher_shared(txn).await?;
    crate::models::queue_job_lifecycles::Entity::record_enqueue(txn, lifecycle).await?;
    if let Value::Object(fields) = &mut payload {
        fields.insert("dispatch_id".into(), Value::String(id.to_string()));
        fields.insert("publishing_epoch".into(), Value::from(epoch));
    }
    record_row(
        txn,
        DispatchRow {
            id,
            lifecycle_id: Some(id),
            payload,
            worker_name,
            queue_name,
            tags,
            publishing_epoch: epoch,
        },
    )
    .await
}

/// # Errors
/// Returns an error if the database operation fails.
pub async fn record_query_in_transaction(
    txn: &impl ConnectionTrait,
    id: Uuid,
    deadline_at: chrono::DateTime<chrono::FixedOffset>,
    mut payload: Value,
    worker_name: String,
    queue_name: Option<String>,
    tags: Option<Vec<String>>,
) -> Result<(), DbErr> {
    let epoch = lock_publisher_shared(txn).await?;
    if let Value::Object(fields) = &mut payload {
        fields.insert("dispatch_id".into(), Value::String(id.to_string()));
        fields.insert("publishing_epoch".into(), Value::from(epoch));
    }
    let tags = tags
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| DbErr::Custom(error.to_string()))?;
    crate::models::_entities::queue_job_dispatch_outbox::ActiveModel {
        id: Set(id),
        lifecycle_id: Set(None),
        workspace_id: Set(payload
            .get("workspace_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())),
        priority: Set(Some(QUERY_EMBEDDING_PRIORITY)),
        payload: Set(payload),
        worker_name: Set(worker_name),
        queue_name: Set(queue_name),
        tags: Set(tags),
        last_attempt_at: Set(None),
        publishing_epoch: Set(epoch),
        deadline_at: Set(Some(deadline_at)),
        published_at: Set(None),
        claim_token: Set(None),
        claim_until: Set(None),
        legacy_quarantined: Set(false),
    }
    .insert(txn)
    .await?;
    Ok(())
}

///
/// # Errors
/// Returns an error if the database operation fails.
pub async fn record_lifecycle_payload(
    txn: &impl ConnectionTrait,
    lifecycle_id: Uuid,
    mut payload: Value,
    worker_name: String,
    queue_name: Option<String>,
    tags: Option<Vec<String>>,
) -> Result<(), DbErr> {
    let epoch = lock_publisher_shared(txn).await?;
    if let Value::Object(fields) = &mut payload {
        fields.insert(
            "lifecycle_id".into(),
            Value::String(lifecycle_id.to_string()),
        );
        fields.insert(
            "dispatch_id".into(),
            Value::String(lifecycle_id.to_string()),
        );
        fields.insert("publishing_epoch".into(), Value::from(epoch));
    }
    let lifecycle_update = crate::models::queue_job_lifecycles::Entity::update_many()
        .col_expr(
            crate::models::_entities::queue_job_lifecycles::Column::Status,
            sea_orm::sea_query::Expr::value(
                crate::models::queue_job_lifecycles::LifecycleStatus::Queued.as_db_str(),
            ),
        )
        .col_expr(
            crate::models::_entities::queue_job_lifecycles::Column::Attempt,
            sea_orm::sea_query::Expr::value(0),
        )
        .filter(crate::models::_entities::queue_job_lifecycles::Column::Id.eq(lifecycle_id))
        .filter(
            crate::models::_entities::queue_job_lifecycles::Column::Status
                .is_in(["retrying", "queued"]),
        )
        .exec(txn)
        .await?;
    if lifecycle_update.rows_affected != 1 {
        return Err(DbErr::RecordNotFound("retryable queue lifecycle".into()));
    }
    crate::models::_entities::queue_job_dispatch_outbox::Entity::update_many()
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::ClaimToken,
            sea_orm::sea_query::Expr::value(None::<String>),
        )
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::ClaimUntil,
            sea_orm::sea_query::Expr::value(None::<chrono::DateTime<chrono::FixedOffset>>),
        )
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::PublishedAt,
            sea_orm::sea_query::Expr::value(None::<chrono::DateTime<chrono::FixedOffset>>),
        )
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::PublishingEpoch,
            sea_orm::sea_query::Expr::value(epoch),
        )
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::Priority,
            sea_orm::sea_query::Expr::value(None::<i32>),
        )
        .filter(crate::models::_entities::queue_job_dispatch_outbox::Column::Id.eq(lifecycle_id))
        .exec(txn)
        .await?;
    record_lifecycle_row(
        txn,
        lifecycle_id,
        payload,
        worker_name,
        queue_name,
        tags,
        epoch,
    )
    .await
}

///
/// # Errors
/// Returns an error if the database operation fails.
async fn record_lifecycle_row(
    db: &impl ConnectionTrait,
    lifecycle_id: Uuid,
    payload: Value,
    worker_name: String,
    queue_name: Option<String>,
    tags: Option<Vec<String>>,
    publishing_epoch: i64,
) -> Result<(), DbErr> {
    let tags = tags
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| DbErr::Custom(error.to_string()))?;
    let result = crate::models::_entities::queue_job_dispatch_outbox::Entity::update_many()
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::PublishingEpoch,
            sea_orm::sea_query::Expr::value(publishing_epoch),
        )
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::Priority,
            sea_orm::sea_query::Expr::value(None::<i32>),
        )
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::WorkspaceId,
            sea_orm::sea_query::Expr::value(
                payload
                    .get("workspace_id")
                    .and_then(Value::as_str)
                    .and_then(|id| Uuid::parse_str(id).ok()),
            ),
        )
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::Payload,
            sea_orm::sea_query::Expr::value(payload),
        )
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::WorkerName,
            sea_orm::sea_query::Expr::value(worker_name),
        )
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::QueueName,
            sea_orm::sea_query::Expr::value(queue_name),
        )
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::Tags,
            sea_orm::sea_query::Expr::value(tags),
        )
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::LastAttemptAt,
            sea_orm::sea_query::Expr::value(None::<chrono::DateTime<chrono::FixedOffset>>),
        )
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::PublishedAt,
            sea_orm::sea_query::Expr::value(None::<chrono::DateTime<chrono::FixedOffset>>),
        )
        .col_expr(
            crate::models::_entities::queue_job_dispatch_outbox::Column::LegacyQuarantined,
            sea_orm::sea_query::Expr::value(false),
        )
        .filter(
            crate::models::_entities::queue_job_dispatch_outbox::Column::LifecycleId
                .eq(Some(lifecycle_id)),
        )
        .exec(db)
        .await?;
    if result.rows_affected == 0 {
        return Err(DbErr::RecordNotFound(
            "lifecycle dispatch outbox row".into(),
        ));
    }
    Ok(())
}

///
/// # Errors
/// Returns an error if the database operation fails.
async fn record_row(db: &impl ConnectionTrait, row: DispatchRow) -> Result<(), DbErr> {
    let tags = row
        .tags
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| DbErr::Custom(error.to_string()))?;
    crate::models::_entities::queue_job_dispatch_outbox::ActiveModel {
        lifecycle_id: Set(row.lifecycle_id),
        id: Set(row.id),
        publishing_epoch: Set(row.publishing_epoch),
        workspace_id: Set(row
            .payload
            .get("workspace_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())),
        payload: Set(row.payload),
        worker_name: Set(row.worker_name),
        queue_name: Set(row.queue_name),
        tags: Set(tags),
        last_attempt_at: Set(None),
        published_at: Set(None),
        claim_token: Set(None),
        claim_until: Set(None),
        priority: Set(None),
        deadline_at: Set(None),
        legacy_quarantined: Set(false),
    }
    .insert(db)
    .await?;
    Ok(())
}

/// Dispatches whose lifecycle is waiting for the queue and whose retry cooldown has passed.
///
/// # Errors
/// Returns an error if the query fails or a stored row cannot be decoded.
pub async fn due(db: &impl ConnectionTrait) -> Result<Vec<PendingDispatch>, YorishiroError> {
    // One batch joins lifecycle state to outbox payloads and compares backend-specific cooldown and retry timestamps.
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
            "(l.retry_at IS NULL OR datetime(l.retry_at) <= datetime('now'))"
        }
        _ => "(l.retry_at IS NULL OR l.retry_at <= CURRENT_TIMESTAMP)",
    };
    let lifecycle_due = format!(
        "(l.id IS NOT NULL AND ((l.status = 'queued' AND o.published_at IS NULL) OR (l.status = 'retrying' AND {retry_due} AND o.published_at IS NULL)))"
    );
    db.query_all_raw(Statement::from_string(
        backend,
        format!(
            "SELECT o.id AS dispatch_id, o.lifecycle_id, l.job_name, l.worker_class, o.payload, o.worker_name, o.queue_name, o.tags, o.publishing_epoch, o.deadline_at, o.workspace_id, o.priority \
         FROM queue_job_dispatch_outbox o LEFT JOIN queue_job_lifecycles l ON o.lifecycle_id = l.id \
          WHERE o.legacy_quarantined = FALSE AND ({lifecycle_due} OR (l.id IS NULL AND o.workspace_id IS NOT NULL AND o.worker_name = 'QueryEmbeddingWorker' AND o.deadline_at > CURRENT_TIMESTAMP AND o.published_at IS NULL)) \
           AND {cooldown} \
          ORDER BY o.id LIMIT {RECOVERY_BATCH_SIZE}"
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
            row.try_get::<Option<Value>>("", "tags")
                .internal()?
                .map(serde_json::from_value)
                .transpose()
                .internal()?
        };
        let worker_class_str = row.try_get::<Option<String>>("", "worker_class").internal()?.unwrap_or_else(|| "shared".to_owned());
        let worker_class = WorkerClass::from_db_str(&worker_class_str).unwrap_or(WorkerClass::Shared);
        Ok(PendingDispatch {
            id: row.try_get("", "dispatch_id").internal()?,
            lifecycle_id: row.try_get("", "lifecycle_id").internal()?,
            job_name: row
                .try_get::<Option<String>>("", "job_name")
                .internal()?
                .unwrap_or_else(|| "query_embedding".to_owned()),
            worker_class,
            payload,
            worker_name: row.try_get("", "worker_name").internal()?,
            queue_name: row.try_get("", "queue_name").internal()?,
            tags,
            publishing_epoch: row.try_get("", "publishing_epoch").internal()?,
            deadline_at: row.try_get("", "deadline_at").internal()?,
            workspace_id: row.try_get("", "workspace_id").internal()?,
            priority: row.try_get("", "priority").internal()?,
        })
    })
    .collect()
}

async fn replay_snapshot_locked(
    txn: &impl ConnectionTrait,
    expected_epoch: i64,
    old_workers_exited: bool,
    snapshot_id: &str,
) -> Result<usize, DbErr> {
    let state = publishing_state(txn).await?;
    if state.active || state.epoch != expected_epoch {
        return Err(DbErr::Custom(
            "replay requires the expected paused publisher epoch".into(),
        ));
    }
    if state.cutoff_at.is_none() {
        return Err(DbErr::Custom(
            "replay requires a recorded cutover cutoff".into(),
        ));
    }
    if state.snapshot_id.as_deref() != Some(snapshot_id) {
        return Err(DbErr::Custom(
            "snapshot evidence must be recorded before replay".into(),
        ));
    }
    let rows = txn.query_all_raw(Statement::from_string(
        txn.get_database_backend(),
        "SELECT o.id FROM queue_job_dispatch_outbox o WHERE o.legacy_quarantined = FALSE AND ((o.lifecycle_id IS NOT NULL AND EXISTS (SELECT 1 FROM queue_job_lifecycles l WHERE l.id = o.lifecycle_id AND l.status IN ('queued', 'retrying', 'running'))) OR (o.lifecycle_id IS NULL AND o.worker_name = 'QueryEmbeddingWorker' AND o.workspace_id IS NOT NULL AND o.deadline_at > CURRENT_TIMESTAMP)) ORDER BY o.id",
    )).await?;
    let unavailable = txn.query_one_raw(Statement::from_string(
        txn.get_database_backend(),
        "SELECT COUNT(*) AS count FROM queue_job_dispatch_outbox o WHERE o.legacy_quarantined = FALSE AND o.lifecycle_id IS NULL AND (o.worker_name <> 'QueryEmbeddingWorker' OR o.workspace_id IS NULL OR o.deadline_at IS NULL) AND o.published_at IS NULL",
    )).await?.ok_or_else(|| DbErr::RecordNotFound("dispatch quarantine scan".into()))?;
    let unavailable_count: i64 = unavailable.try_get("", "count")?;
    if unavailable_count != 0 {
        return Err(DbErr::Custom(format!(
            "{unavailable_count} lifecycle-less dispatch rows cannot be replayed safely"
        )));
    }
    let mut ids = Vec::with_capacity(rows.len());
    for row in rows {
        ids.push(row.try_get("", "id")?);
    }
    ids.sort_unstable();
    ids.dedup();
    let mut replayed = 0;
    let next_epoch = expected_epoch + 1;
    for &id in &ids {
        if retag_for_replay_locked(txn, id, next_epoch, old_workers_exited, snapshot_id)
            .await?
            .is_some()
        {
            replayed += 1;
        }
    }
    Ok(replayed)
}

///
/// # Errors
/// Returns an error if the database operation fails.
pub async fn prepare_replay_snapshot(
    txn: &impl ConnectionTrait,
    expected_epoch: i64,
    cutoff_at: chrono::DateTime<chrono::FixedOffset>,
    old_workers_exited: bool,
    snapshot_id: &str,
) -> Result<usize, DbErr> {
    if !old_workers_exited || snapshot_id.trim().is_empty() {
        return Err(DbErr::Custom(
            "replay requires old-worker exit attestation and snapshot id".into(),
        ));
    }
    lock_publisher_exclusive(txn).await?;
    let state = publishing_state(txn).await?;
    if state.active || state.epoch != expected_epoch {
        return Err(DbErr::Custom(
            "replay requires the expected paused publisher epoch".into(),
        ));
    }
    if state.snapshot_id.is_some() {
        return Err(DbErr::Custom(
            "replay snapshot has already been prepared for this epoch".into(),
        ));
    }
    use crate::models::_entities::queue_publishing_state::{Column, Entity};
    let cutoff = Entity::update_many()
        .col_expr(Column::CutoffAt, Expr::val(cutoff_at))
        .col_expr(Column::SnapshotId, Expr::val(snapshot_id))
        .filter(Column::Id.eq(1))
        .filter(Column::Epoch.eq(expected_epoch))
        .filter(Column::Active.eq(false))
        .exec(txn)
        .await?;
    if cutoff.rows_affected != 1 {
        return Err(DbErr::RecordNotFound(
            "paused publisher state changed".into(),
        ));
    }
    let replayed =
        replay_snapshot_locked(txn, expected_epoch, old_workers_exited, snapshot_id).await?;
    let advanced = Entity::update_many()
        .col_expr(Column::Epoch, Expr::val(expected_epoch + 1))
        .filter(Column::Id.eq(1))
        .filter(Column::Epoch.eq(expected_epoch))
        .filter(Column::Active.eq(false))
        .filter(Column::SnapshotId.eq(snapshot_id))
        .exec(txn)
        .await?;
    if advanced.rows_affected != 1 {
        return Err(DbErr::RecordNotFound(
            "prepared publisher epoch changed".into(),
        ));
    }
    Ok(replayed)
}

/// # Errors
/// Returns an error if the database operation fails.
pub async fn activate_prepared_publishing_epoch(
    txn: &impl ConnectionTrait,
    expected_epoch: i64,
    snapshot_id: &str,
) -> Result<(), DbErr> {
    lock_publisher_exclusive(txn).await?;
    let state = publishing_state(txn).await?;
    if state.active
        || state.epoch != expected_epoch + 1
        || state.cutoff_at.is_none()
        || state.snapshot_id.as_deref() != Some(snapshot_id)
    {
        return Err(DbErr::Custom(
            "activation requires the matching prepared next epoch and snapshot evidence".into(),
        ));
    }
    use crate::models::_entities::queue_publishing_state::{Column, Entity};
    let result = Entity::update_many()
        .col_expr(Column::Active, Expr::val(true))
        .filter(Column::Id.eq(1))
        .filter(Column::Epoch.eq(expected_epoch + 1))
        .filter(Column::Active.eq(false))
        .filter(Column::SnapshotId.eq(snapshot_id))
        .exec(txn)
        .await?;
    if result.rows_affected != 1 {
        return Err(DbErr::RecordNotFound(
            "prepared publishing epoch changed".into(),
        ));
    }
    Ok(())
}

/// Retags one pending dispatch after the old worker pool has exited.
///
/// # Errors
/// Returns an error if the database operation fails.
pub async fn retag_for_replay(
    txn: &impl ConnectionTrait,
    id: Uuid,
    epoch: i64,
    old_workers_exited: bool,
    queue_snapshot_id: &str,
) -> Result<Option<PendingDispatch>, DbErr> {
    if !old_workers_exited || queue_snapshot_id.trim().is_empty() {
        return Ok(None);
    }
    lock_publisher_exclusive(txn).await?;
    let state = publishing_state(txn).await?;
    if state.active || state.epoch != epoch {
        return Ok(None);
    }
    retag_for_replay_locked(txn, id, epoch, old_workers_exited, queue_snapshot_id).await
}

///
/// # Errors
/// Returns an error if the database operation fails.
pub async fn publishing_state(db: &impl ConnectionTrait) -> Result<PublishingState, DbErr> {
    use crate::models::_entities::queue_publishing_state::Entity;
    let row = Entity::find_by_id(1)
        .one(db)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("queue publishing state".into()))?;
    Ok(PublishingState {
        epoch: row.epoch,
        active: row.active,
        cutoff_at: row.cutoff_at,
        snapshot_id: row.snapshot_id,
    })
}

///
/// # Errors
/// Returns an error if the database operation fails.
///
/// # Errors
/// Returns an error if the database operation fails.
pub async fn change_publishing_state(
    txn: &impl ConnectionTrait,
    expected_epoch: i64,
    active: bool,
    cutoff_at: Option<chrono::DateTime<chrono::FixedOffset>>,
) -> Result<i64, DbErr> {
    lock_publisher_exclusive(txn).await?;
    use crate::models::_entities::queue_publishing_state::{Column, Entity};
    let result = Entity::update_many()
        .col_expr(Column::Epoch, Expr::col(Column::Epoch).add(1))
        .col_expr(Column::Active, Expr::val(active))
        .col_expr(Column::CutoffAt, Expr::val(cutoff_at))
        .col_expr(Column::SnapshotId, Expr::val(None::<String>))
        .filter(Column::Id.eq(1))
        .filter(Column::Epoch.eq(expected_epoch))
        .exec(txn)
        .await?;
    if result.rows_affected != 1 {
        return Err(DbErr::RecordNotFound(
            "queue publishing epoch changed".into(),
        ));
    }
    Ok(expected_epoch + 1)
}

/// Claims an unpublished lifecycle row within the active publisher epoch.
///
/// # Errors
/// Returns an error if the claim transaction fails.
pub async fn claim_for_publish(
    txn: &impl ConnectionTrait,
    id: Uuid,
    expected_epoch: i64,
) -> Result<bool, DbErr> {
    let state = publishing_state(txn).await?;
    if !state.active || state.epoch != expected_epoch {
        return Ok(false);
    }
    use crate::models::_entities::queue_job_dispatch_outbox::{Column, Entity};
    let result = Entity::update_many()
        .col_expr(Column::LastAttemptAt, Expr::current_timestamp())
        .filter(Column::Id.eq(id))
        .filter(Column::PublishingEpoch.eq(expected_epoch))
        .filter(Column::PublishedAt.is_null())
        .exec(txn)
        .await?;
    Ok(result.rows_affected == 1)
}

/// Advances the publisher epoch while applying the requested pause or activation state.
///
/// # Errors
/// Returns an error if the expected epoch no longer matches.
pub async fn advance_publishing_epoch(
    txn: &impl ConnectionTrait,
    expected_epoch: i64,
    active: bool,
    cutoff_at: Option<&str>,
) -> Result<i64, DbErr> {
    let cutoff = cutoff_at
        .map(chrono::DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|error| DbErr::Custom(format!("invalid cutover timestamp: {error}")))?;
    change_publishing_state(txn, expected_epoch, active, cutoff).await
}

/// Records the operator-attested queue cutoff for a paused epoch.
///
/// # Errors
/// Returns an error if the publisher epoch is not paused or the update fails.
pub async fn set_cutover_cutoff(
    txn: &impl ConnectionTrait,
    expected_epoch: i64,
    cutoff_at: chrono::DateTime<chrono::FixedOffset>,
) -> Result<(), DbErr> {
    lock_publisher_exclusive(txn).await?;
    use crate::models::_entities::queue_publishing_state::{Column, Entity};
    let result = Entity::update_many()
        .col_expr(Column::CutoffAt, Expr::val(cutoff_at))
        .filter(Column::Id.eq(1))
        .filter(Column::Epoch.eq(expected_epoch))
        .filter(Column::Active.eq(false))
        .exec(txn)
        .await?;
    if result.rows_affected != 1 {
        return Err(DbErr::RecordNotFound(
            "paused publishing epoch changed".into(),
        ));
    }
    Ok(())
}

/// # Errors
/// Returns an error if the database operation fails.
pub async fn claim_dispatch(
    db: &DatabaseConnection,
    id: Uuid,
    expected_epoch: i64,
    claim_token: Uuid,
    lease_seconds: i64,
) -> Result<bool, DbErr> {
    let txn = db.begin().await?;
    lock_publisher_exclusive(&txn).await?;
    let state = publishing_state(&txn).await?;
    if !state.active || state.epoch != expected_epoch {
        txn.rollback().await?;
        return Ok(false);
    }
    use crate::models::_entities::queue_job_dispatch_outbox::{Column, Entity};
    let lease_until = crate::time::now() + chrono::Duration::seconds(lease_seconds);
    let result = Entity::update_many()
        .col_expr(Column::LastAttemptAt, Expr::current_timestamp())
        .col_expr(Column::ClaimToken, Expr::val(claim_token.to_string()))
        .col_expr(Column::ClaimUntil, Expr::val(lease_until))
        .filter(Column::Id.eq(id))
        .filter(Column::PublishingEpoch.eq(expected_epoch))
        .filter(Column::PublishedAt.is_null())
        .filter(
            Column::ClaimUntil
                .is_null()
                .or(Expr::col(Column::ClaimUntil).lte(Expr::current_timestamp())),
        )
        .exec(&txn)
        .await?;
    let claimed = result.rows_affected == 1;
    txn.commit().await?;
    if !claimed {
        return Ok(false);
    }
    Ok(true)
}

///
/// # Errors
/// Returns an error if the database operation fails.
pub async fn acknowledge_dispatch(
    db: &DatabaseConnection,
    id: Uuid,
    epoch: i64,
    claim_token: Uuid,
    provider_job_id: &str,
) -> Result<bool, DbErr> {
    let txn = db.begin().await?;
    lock_publisher_exclusive(&txn).await?;
    let state = publishing_state(&txn).await?;
    if !state.active || state.epoch != epoch {
        txn.rollback().await?;
        return Ok(false);
    }
    use crate::models::_entities::queue_job_dispatch_outbox::{Column, Entity};
    let result = Entity::update_many()
        .col_expr(Column::PublishedAt, Expr::current_timestamp())
        .col_expr(Column::ClaimToken, Expr::val(None::<String>))
        .col_expr(
            Column::ClaimUntil,
            Expr::val(None::<chrono::DateTime<chrono::FixedOffset>>),
        )
        .filter(Column::Id.eq(id))
        .filter(Column::PublishingEpoch.eq(epoch))
        .filter(Column::ClaimToken.eq(claim_token.to_string()))
        .filter(Expr::col(Column::ClaimUntil).gt(Expr::current_timestamp()))
        .filter(Column::PublishedAt.is_null())
        .exec(&txn)
        .await?;
    if result.rows_affected != 1 {
        txn.rollback().await?;
        return Ok(false);
    }
    if let Some(row) = crate::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
        .one(&txn)
        .await?
        && let Some(lifecycle_id) = row.lifecycle_id
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

///
/// # Errors
/// Returns an error if the database operation fails.
///
/// # Errors
/// Returns an error if the database operation fails.
async fn retag_for_replay_locked(
    txn: &impl ConnectionTrait,
    id: Uuid,
    epoch: i64,
    old_workers_exited: bool,
    queue_snapshot_id: &str,
) -> Result<Option<PendingDispatch>, DbErr> {
    if !old_workers_exited || queue_snapshot_id.trim().is_empty() {
        return Ok(None);
    }
    let Some(row) = crate::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
        .one(txn)
        .await?
    else {
        return Ok(None);
    };
    let Some(lifecycle_id) = row.lifecycle_id else {
        if row.worker_name != crate::workers::query_embedding::QueryEmbeddingWorker::class_name()
            || row.workspace_id.is_none()
            || row
                .deadline_at
                .is_none_or(|deadline| deadline <= crate::time::now())
            || row.legacy_quarantined
        {
            return Ok(None);
        }
        let request_id = row
            .payload
            .get("request_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok());
        let Some(request_id) = request_id else {
            return Ok(None);
        };
        let query_request = crate::models::query_embedding_requests::Entity::find_by_id(request_id)
            .filter(
                crate::models::query_embedding_requests::Column::WorkspaceId
                    .eq(row.workspace_id.unwrap_or_default()),
            )
            .one(txn)
            .await?;
        let Some(query_request) = query_request else {
            return Ok(None);
        };
        let now = crate::time::now();
        if query_request.status != "pending"
            || query_request.expires_at <= now
            || query_request
                .deadline_at
                .is_none_or(|deadline| deadline <= now)
        {
            return Ok(None);
        }
        let mut active = row.into_active_model();
        active.publishing_epoch = Set(epoch);
        active.last_attempt_at = Set(None);
        active.published_at = Set(None);
        active.claim_token = Set(None);
        active.claim_until = Set(None);
        let mut payload = active.payload.clone().unwrap();
        if let Value::Object(fields) = &mut payload {
            fields.insert("dispatch_id".into(), Value::String(id.to_string()));
            fields.insert("publishing_epoch".into(), Value::from(epoch));
        }
        let pending = PendingDispatch {
            id,
            lifecycle_id: None,
            job_name: "query_embedding".into(),
            worker_class: WorkerClass::Shared,
            payload: payload.clone(),
            worker_name: active.worker_name.clone().unwrap(),
            queue_name: active.queue_name.clone().unwrap(),
            tags: active
                .tags
                .clone()
                .unwrap()
                .and_then(|v| serde_json::from_value(v).ok()),
            publishing_epoch: epoch,
            deadline_at: active.deadline_at.clone().unwrap(),
            workspace_id: active.workspace_id.clone().unwrap(),
            priority: active.priority.clone().unwrap(),
        };
        active.payload = Set(payload);
        active.update(txn).await?;
        return Ok(Some(pending));
    };
    let lifecycle =
        crate::models::_entities::queue_job_lifecycles::Entity::find_by_id(lifecycle_id)
            .one(txn)
            .await?;
    let Some(lifecycle) = lifecycle else {
        return Ok(None);
    };
    if !matches!(lifecycle.status.as_str(), "queued" | "retrying" | "running") {
        return Ok(None);
    }
    if row.workspace_id.is_none() || row.legacy_quarantined {
        return Ok(None);
    }
    crate::models::queue_job_lifecycles::Entity::update_many()
        .col_expr(
            crate::models::_entities::queue_job_lifecycles::Column::ProviderJobId,
            sea_orm::sea_query::Expr::value(None::<String>),
        )
        .filter(crate::models::_entities::queue_job_lifecycles::Column::Id.eq(lifecycle_id))
        .filter(
            crate::models::_entities::queue_job_lifecycles::Column::Status
                .eq(lifecycle.status.clone()),
        )
        .exec(txn)
        .await?;
    if lifecycle.status == "running" {
        crate::models::queue_job_lifecycles::Entity::update_many()
            .col_expr(
                crate::models::_entities::queue_job_lifecycles::Column::Status,
                sea_orm::sea_query::Expr::value("queued"),
            )
            .col_expr(
                crate::models::_entities::queue_job_lifecycles::Column::Attempt,
                sea_orm::sea_query::Expr::col(
                    crate::models::_entities::queue_job_lifecycles::Column::Attempt,
                )
                .add(1),
            )
            .col_expr(
                crate::models::_entities::queue_job_lifecycles::Column::ProviderJobId,
                sea_orm::sea_query::Expr::value(None::<String>),
            )
            .col_expr(
                crate::models::_entities::queue_job_lifecycles::Column::LeaseUntil,
                sea_orm::sea_query::Expr::value(None::<chrono::DateTime<chrono::FixedOffset>>),
            )
            .col_expr(
                crate::models::_entities::queue_job_lifecycles::Column::ClaimAt,
                sea_orm::sea_query::Expr::value(None::<chrono::DateTime<chrono::FixedOffset>>),
            )
            .filter(crate::models::_entities::queue_job_lifecycles::Column::Id.eq(lifecycle_id))
            .filter(crate::models::_entities::queue_job_lifecycles::Column::Status.eq("running"))
            .exec(txn)
            .await?;
    }
    let mut active = row.into_active_model();
    active.publishing_epoch = Set(epoch);
    active.last_attempt_at = Set(None);
    active.published_at = Set(None);
    active.claim_token = Set(None);
    active.claim_until = Set(None);
    let mut payload = active.payload.clone().unwrap();
    if let Value::Object(fields) = &mut payload {
        fields.insert("dispatch_id".into(), Value::String(id.to_string()));
        fields.insert("publishing_epoch".into(), Value::from(epoch));
    }
    let worker_name = active.worker_name.clone().unwrap();
    let queue_name = active.queue_name.clone().unwrap();
    let tags = active
        .tags
        .clone()
        .unwrap()
        .and_then(|v| serde_json::from_value(v).ok());
    let workspace_id = active.workspace_id.clone().unwrap();
    let priority = active.priority.clone().unwrap();
    active.payload = Set(payload.clone());
    active.update(txn).await?;
    Ok(Some(PendingDispatch {
        id,
        lifecycle_id: Some(lifecycle_id),
        job_name: lifecycle.job_name,
        worker_class: WorkerClass::from_db_str(&lifecycle.worker_class)
            .unwrap_or(WorkerClass::Shared),
        payload,
        worker_name,
        queue_name,
        tags,
        publishing_epoch: epoch,
        deadline_at: None,
        workspace_id,
        priority,
    }))
}

///
/// # Errors
/// Returns an error if the database operation fails.
///
/// # Errors
/// Returns an error if the database operation fails.
pub(crate) async fn expire(db: &impl ConnectionTrait, id: Uuid) -> Result<(), DbErr> {
    crate::models::_entities::queue_job_dispatch_outbox::Entity::delete_by_id(id)
        .exec(db)
        .await?;
    Ok(())
}

/// Counts unfinished dispatches that still use the retired tag routing.
///
/// # Errors
/// Returns an error if the query fails, including when the schema has not been migrated.
pub async fn count_retired_routes(db: &impl ConnectionTrait) -> Result<u64, DbErr> {
    let backend = db.get_database_backend();
    // Raw SQL expresses JSON array membership on both backends without matching serialized substrings.
    let current_target = match backend {
        sea_orm::DatabaseBackend::Sqlite => {
            "EXISTS (SELECT 1 FROM json_each(o.tags) t WHERE substr(t.value, 1, 14) = 'worker-target:')"
        }
        _ => {
            "EXISTS (SELECT 1 FROM jsonb_array_elements_text(o.tags) t(value) WHERE left(t.value, 14) = 'worker-target:')"
        }
    };
    let row = db
        .query_one_raw(Statement::from_string(
            backend,
            format!(
                "SELECT COUNT(*) AS retired FROM queue_job_dispatch_outbox o \
                 JOIN queue_job_lifecycles l ON l.id = o.lifecycle_id \
                 WHERE l.status IN ('queued', 'running', 'retrying') \
                 AND o.tags IS NOT NULL AND NOT ({current_target})"
            ),
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("retired route count returned no row".into()))?;
    let count: i64 = row.try_get("", "retired")?;
    u64::try_from(count).map_err(|error| DbErr::Type(error.to_string()))
}
