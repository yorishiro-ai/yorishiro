pub use crate::models::_entities::queue_job_lifecycles::{ActiveModel, Column, Entity, Model};
use chrono::Utc;
use sea_orm::ActiveValue::Set;
use sea_orm::entity::prelude::*;
use sea_orm::{
    DatabaseBackend, DatabaseTransaction, ExprTrait, IntoActiveModel, PaginatorTrait, QuerySelect,
    SqliteTransactionMode, TransactionOptions, TransactionTrait,
};
use tokio::task::JoinHandle;

use crate::db_enum::db_enum;

const HEARTBEAT_INTERVAL_SECONDS: u64 = 60;
const LEASE_DURATION_MINUTES: i64 = 5;

db_enum! {
    /// Where a queued job stands.
    /// Matches `status`'s CHECK constraint string-for-string.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum LifecycleStatus {
        Queued = "queued",
        Running = "running",
        Retrying = "retrying",
        Completed = "completed",
        Failed = "failed",
        Cancelled = "cancelled",
        Unavailable = "unavailable",
    }
}

impl LifecycleStatus {
    /// A stored status this crate does not define is a corrupt row, not a missing one.
    pub fn from_row(value: &str) -> Result<Self, DbErr> {
        Self::from_db_str(value)
            .ok_or_else(|| DbErr::Type(format!("unknown queue lifecycle status {value:?}")))
    }
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
    pub(crate) async fn count_waiting_lower_classes(
        db: &impl ConnectionTrait,
        lower_classes: &[crate::workers::embedding_sync::WorkerClass],
        cutoff: DateTimeWithTimeZone,
    ) -> Result<u64, DbErr> {
        let lower_classes = lower_classes
            .iter()
            .copied()
            .map(crate::workers::embedding_sync::WorkerClass::as_db_str);
        Entity::find()
            .filter(Column::Status.is_in([
                LifecycleStatus::Queued.as_db_str(),
                LifecycleStatus::Retrying.as_db_str(),
            ]))
            .filter(Column::WorkerClass.is_in(lower_classes))
            .filter(Column::EnqueueAt.lte(cutoff))
            .count(db)
            .await
    }

    pub async fn record_enqueue(
        db: &impl ConnectionTrait,
        enqueue: Enqueue<'_>,
    ) -> Result<(), DbErr> {
        ActiveModel {
            id: Set(enqueue.id),
            provider_job_id: Set(None),
            job_name: Set(enqueue.job_name.to_owned()),
            worker_class: Set(enqueue.worker_class.as_db_str().to_owned()),
            workspace_id: Set(enqueue.workspace_id),
            plan: Set(enqueue.plan.map(str::to_owned)),
            status: Set(LifecycleStatus::Queued.as_db_str().to_owned()),
            enqueue_at: Set(Utc::now().fixed_offset()),
            claim_at: Set(None),
            start_at: Set(None),
            retry_at: Set(None),
            completed_at: Set(None),
            failed_at: Set(None),
            cancelled_at: Set(None),
            admitted_at: Set(None),
            lease_until: Set(None),
            attempt: Set(0),
            concurrency_key: Set(enqueue.concurrency_key.map(str::to_owned)),
            concurrency_limit: Set(enqueue.concurrency_limit),
            error: Set(None),
            ..Default::default()
        }
        .insert(db)
        .await
        .map(|_| ())
    }

    pub(crate) async fn mark_dispatched(
        db: &impl ConnectionTrait,
        id: Uuid,
        provider_job_id: &str,
    ) -> Result<(), DbErr> {
        let mut row = Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(DbErr::RecordNotFound("queue lifecycle".into()))?
            .into_active_model();
        row.provider_job_id = Set(Some(provider_job_id.to_owned()));
        row.update(db).await.map(|_| ())
    }

    pub async fn defer(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: Option<i32>,
        error: &str,
    ) -> Result<(), DbErr> {
        let now = Utc::now().fixed_offset();
        let result = Entity::update_many()
            .col_expr(
                Column::Status,
                Expr::value(LifecycleStatus::Retrying.as_db_str()),
            )
            .col_expr(Column::RetryAt, Expr::value(now))
            .col_expr(
                Column::LeaseUntil,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(Column::Error, Expr::value(Some(error.to_owned())))
            .filter(Column::Id.eq(id))
            .filter(if let Some(attempt) = attempt {
                Column::Attempt
                    .eq(attempt)
                    .and(Column::Status.eq(LifecycleStatus::Running.as_db_str()))
            } else {
                Column::Status.is_in([
                    LifecycleStatus::Queued.as_db_str(),
                    LifecycleStatus::Retrying.as_db_str(),
                ])
            })
            .exec(db)
            .await?;
        tracing::warn!(lifecycle_id = %id, diagnostic = error, "queue job deferred for retry");
        if result.rows_affected == 0 {
            return Err(DbErr::RecordNotFound("active queue lease".into()));
        }
        Ok(())
    }

    pub async fn defer_at(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: i32,
        error: &str,
        now: DateTimeWithTimeZone,
    ) -> Result<(), DbErr> {
        let result = Entity::update_many()
            .col_expr(
                Column::Status,
                Expr::value(LifecycleStatus::Retrying.as_db_str()),
            )
            .col_expr(Column::RetryAt, Expr::value(now))
            .col_expr(
                Column::LeaseUntil,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(Column::Error, Expr::value(Some(error.to_owned())))
            .filter(Column::Id.eq(id))
            .filter(Column::Attempt.eq(attempt))
            .filter(Column::Status.eq(LifecycleStatus::Running.as_db_str()))
            .filter(
                Column::LeaseUntil
                    .is_not_null()
                    .and(Column::LeaseUntil.lte(now)),
            )
            .exec(db)
            .await?;
        tracing::warn!(lifecycle_id = %id, diagnostic = error, "queue job deferred for retry");
        if result.rows_affected == 0 {
            return Err(DbErr::RecordNotFound("active queue lease".into()));
        }
        Ok(())
    }

    pub(crate) async fn renew(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: i32,
        now: DateTimeWithTimeZone,
    ) -> Result<bool, DbErr> {
        Self::renew_at(db, id, attempt, now).await
    }

    pub async fn renew_at(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: i32,
        now: DateTimeWithTimeZone,
    ) -> Result<bool, DbErr> {
        let result = Entity::update_many()
            .col_expr(Column::LeaseUntil, Expr::value(now + lease_duration()))
            .filter(Column::Id.eq(id))
            .filter(Column::Attempt.eq(attempt))
            .filter(Column::Status.eq(LifecycleStatus::Running.as_db_str()))
            .exec(db)
            .await?;
        Ok(result.rows_affected == 1)
    }

    pub(crate) fn heartbeat(db: DatabaseConnection, id: Uuid, attempt: i32) -> Heartbeat {
        Heartbeat(tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECONDS));
            ticker.tick().await;
            loop {
                match Self::renew(&db, id, attempt, Utc::now().fixed_offset()).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        tracing::warn!(lifecycle_id = %id, diagnostic = %error, "queue lease renewal failed");
                        break;
                    }
                }
                ticker.tick().await;
            }
        }))
    }

    pub(crate) async fn start(db: &DatabaseConnection, id: Uuid) -> Result<Admission, DbErr> {
        Self::start_at(db, id, Utc::now().fixed_offset()).await
    }

    pub async fn start_at(
        db: &DatabaseConnection,
        id: Uuid,
        now: DateTimeWithTimeZone,
    ) -> Result<Admission, DbErr> {
        let txn = if db.get_database_backend() == DatabaseBackend::Sqlite {
            db.begin_with_options(TransactionOptions {
                sqlite_transaction_mode: Some(SqliteTransactionMode::Immediate),
                ..Default::default()
            })
            .await?
        } else {
            db.begin().await?
        };
        let admission = admit(&txn, id, now).await?;
        // Only an admission wrote anything; every other outcome leaves the row as it found it.
        match admission {
            Admission::Started { .. } | Admission::Recovered { .. } => txn.commit().await?,
            _ => txn.rollback().await?,
        }
        Ok(admission)
    }

    pub async fn finish(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: Option<i32>,
        status: LifecycleStatus,
        error: Option<&str>,
    ) -> Result<(), DbErr> {
        let now = Utc::now().fixed_offset();
        let mut update = Entity::update_many()
            .col_expr(Column::Status, Expr::value(status.as_db_str()))
            .col_expr(
                Column::LeaseUntil,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(Column::Error, Expr::value(error.map(str::to_owned)))
            .filter(Column::Id.eq(id));
        if let Some(attempt) = attempt {
            update = update
                .filter(Column::Attempt.eq(attempt))
                .filter(Column::Status.eq(LifecycleStatus::Running.as_db_str()));
        }
        match status {
            LifecycleStatus::Completed => {
                update = update.col_expr(Column::CompletedAt, Expr::value(now));
            }
            LifecycleStatus::Failed => {
                update = update.col_expr(Column::FailedAt, Expr::value(now));
            }
            LifecycleStatus::Retrying => {
                update = update.col_expr(Column::RetryAt, Expr::value(now));
            }
            LifecycleStatus::Cancelled => {
                update = update.col_expr(Column::CancelledAt, Expr::value(now));
            }
            _ => {}
        }
        let result = update.exec(db).await?;
        if result.rows_affected == 0 {
            return Err(DbErr::RecordNotFound("active queue lease".into()));
        }
        Ok(())
    }
}

/// Decides, under the row lock, whether one delivery of a lifecycle may run now.
///
/// Statuses are mutually exclusive, so the order of the checks below only decides which outcome is reported first, never which is correct.
async fn admit(
    txn: &DatabaseTransaction,
    id: Uuid,
    now: DateTimeWithTimeZone,
) -> Result<Admission, DbErr> {
    let row = Entity::find_by_id(id)
        .lock_exclusive()
        .one(txn)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("queue lifecycle".into()))?;
    let status = LifecycleStatus::from_row(&row.status)?;
    match status {
        LifecycleStatus::Completed | LifecycleStatus::Cancelled => return Ok(Admission::Terminal),
        LifecycleStatus::Running if running_lease_is_live(row.lease_until, now) => {
            return Ok(Admission::Duplicate {
                attempt: row.attempt,
            });
        }
        LifecycleStatus::Running => {
            tracing::warn!(lifecycle_id = %id, "reclaiming expired queue lease");
        }
        _ => {}
    }
    let recovered = status == LifecycleStatus::Running;

    let in_flight = match &row.concurrency_key {
        Some(key) => {
            crate::db::lock_for_update(txn, &format!("queue-concurrency:{key}")).await?;
            Some(in_flight(txn, key, now, id).await?)
        }
        None => None,
    };
    if let (Some(in_flight), Some(limit)) = (in_flight, row.concurrency_limit)
        && in_flight >= u64::try_from(limit).unwrap_or(0)
    {
        tracing::warn!(
            lifecycle_id = %id,
            worker_class = %row.worker_class,
            concurrency_key = ?row.concurrency_key,
            active_capacity = in_flight,
            capacity_limit = limit,
            "queue capacity saturated"
        );
        return Ok(Admission::Saturated {
            attempt: recovered.then_some(row.attempt),
        });
    }

    if claim(txn, id, recovered, now).await? != 1 {
        return Ok(Admission::Duplicate {
            attempt: row.attempt,
        });
    }
    tracing::info!(
        lifecycle_id = %id,
        worker_class = %row.worker_class,
        // The claimed row now counts toward its own key.
        active_capacity = ?in_flight.map(|count| count + 1),
        capacity_limit = ?row.concurrency_limit,
        queue_start_seconds = std::cmp::Ord::max(now.signed_duration_since(row.enqueue_at).num_seconds(), 0),
        "queue job admitted"
    );
    let attempt = row.attempt + 1;
    Ok(if recovered {
        Admission::Recovered { attempt }
    } else {
        Admission::Started { attempt }
    })
}

/// Attempts running under `key` whose lease has not lapsed, not counting `except`.
async fn in_flight(
    db: &impl ConnectionTrait,
    key: &str,
    now: DateTimeWithTimeZone,
    except: Uuid,
) -> Result<u64, DbErr> {
    Entity::find()
        .filter(Column::ConcurrencyKey.eq(key))
        .filter(Column::Status.eq(LifecycleStatus::Running.as_db_str()))
        .filter(Column::LeaseUntil.is_null().or(Column::LeaseUntil.gt(now)))
        .filter(Column::Id.ne(except))
        .count(db)
        .await
}

/// Moves the row to `running` for a new attempt and returns how many rows that changed.
/// Reclaiming an expired lease may only take a row that is still running; a fresh claim may only take one that is waiting.
async fn claim(
    db: &impl ConnectionTrait,
    id: Uuid,
    recovered: bool,
    now: DateTimeWithTimeZone,
) -> Result<u64, DbErr> {
    let claimable = if recovered {
        Column::Status.eq(LifecycleStatus::Running.as_db_str())
    } else {
        Column::Status.is_in([
            LifecycleStatus::Queued.as_db_str(),
            LifecycleStatus::Failed.as_db_str(),
            LifecycleStatus::Retrying.as_db_str(),
        ])
    };
    let result = Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(LifecycleStatus::Running.as_db_str()),
        )
        .col_expr(Column::ClaimAt, Expr::value(now))
        .col_expr(Column::StartAt, Expr::value(now))
        .col_expr(Column::AdmittedAt, Expr::value(now))
        .col_expr(
            Column::FailedAt,
            Expr::value(Option::<DateTimeWithTimeZone>::None),
        )
        .col_expr(
            Column::RetryAt,
            Expr::value(Option::<DateTimeWithTimeZone>::None),
        )
        .col_expr(Column::LeaseUntil, Expr::value(now + lease_duration()))
        .col_expr(Column::Attempt, Expr::col(Column::Attempt).add(1))
        .filter(Column::Id.eq(id))
        .filter(claimable)
        .exec(db)
        .await?;
    Ok(result.rows_affected)
}

/// Keeps one running attempt's lease alive until it is dropped.
///
/// Stopping on drop, not only on an explicit call, means a cancelled or panicking worker cannot leave a renewal task extending the lease of a job nobody is running.
pub(crate) struct Heartbeat(JoinHandle<()>);

impl Heartbeat {
    pub(crate) fn abort(self) {
        drop(self);
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Started { attempt: i32 },
    Recovered { attempt: i32 },
    Duplicate { attempt: i32 },
    Saturated { attempt: Option<i32> },
    Terminal,
}

pub fn running_lease_is_live(
    lease_until: Option<DateTimeWithTimeZone>,
    now: DateTimeWithTimeZone,
) -> bool {
    lease_until.is_none_or(|lease| lease > now)
}

pub fn lease_duration() -> chrono::Duration {
    chrono::Duration::minutes(LEASE_DURATION_MINUTES)
}

pub struct Enqueue<'a> {
    pub id: Uuid,
    pub job_name: &'a str,
    pub worker_class: crate::workers::embedding_sync::WorkerClass,
    pub workspace_id: Option<Uuid>,
    pub plan: Option<&'a str>,
    pub concurrency_key: Option<&'a str>,
    pub concurrency_limit: Option<i32>,
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}
