pub(crate) use crate::models::_entities::queue_job_lifecycles::{
    ActiveModel, Column, Entity, Model,
};
use chrono::Utc;
use sea_orm::ActiveValue::Set;
use sea_orm::entity::prelude::*;
use sea_orm::{ExprTrait, IntoActiveModel, PaginatorTrait, QuerySelect, TransactionTrait};
use tokio::task::JoinHandle;

const HEARTBEAT_INTERVAL_SECONDS: u64 = 60;
const LEASE_DURATION_MINUTES: i64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleStatus {
    Queued,
    Running,
    Retrying,
    Completed,
    Failed,
    Cancelled,
    Unavailable,
}

impl LifecycleStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Retrying => "retrying",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unavailable => "unavailable",
        }
    }

    fn from_db(value: &str) -> Result<Self, DbErr> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "retrying" => Ok(Self::Retrying),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "unavailable" => Ok(Self::Unavailable),
            other => Err(DbErr::Type(format!(
                "unknown queue lifecycle status {other:?}"
            ))),
        }
    }

    #[cfg(test)]
    const ALL: [Self; 7] = [
        Self::Queued,
        Self::Running,
        Self::Retrying,
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
        Self::Unavailable,
    ];
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
                LifecycleStatus::Queued.as_str(),
                LifecycleStatus::Retrying.as_str(),
            ]))
            .filter(Column::WorkerClass.is_in(lower_classes))
            .filter(Column::EnqueueAt.lte(cutoff))
            .count(db)
            .await
    }

    pub(crate) async fn record_enqueue(
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
            status: Set(LifecycleStatus::Queued.as_str().to_owned()),
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

    pub(crate) async fn defer(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: Option<i32>,
        error: &str,
    ) -> Result<(), DbErr> {
        let now = Utc::now().fixed_offset();
        let result = Entity::update_many()
            .col_expr(
                Column::Status,
                Expr::value(LifecycleStatus::Retrying.as_str()),
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
                    .and(Column::Status.eq(LifecycleStatus::Running.as_str()))
            } else {
                Column::Status.is_in([
                    LifecycleStatus::Queued.as_str(),
                    LifecycleStatus::Retrying.as_str(),
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

    pub(crate) async fn defer_at(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: i32,
        error: &str,
        now: DateTimeWithTimeZone,
    ) -> Result<(), DbErr> {
        let result = Entity::update_many()
            .col_expr(
                Column::Status,
                Expr::value(LifecycleStatus::Retrying.as_str()),
            )
            .col_expr(Column::RetryAt, Expr::value(now))
            .col_expr(
                Column::LeaseUntil,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(Column::Error, Expr::value(Some(error.to_owned())))
            .filter(Column::Id.eq(id))
            .filter(Column::Attempt.eq(attempt))
            .filter(Column::Status.eq(LifecycleStatus::Running.as_str()))
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

    pub(crate) async fn renew_at(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: i32,
        now: DateTimeWithTimeZone,
    ) -> Result<bool, DbErr> {
        let result = Entity::update_many()
            .col_expr(Column::LeaseUntil, Expr::value(now + lease_duration()))
            .filter(Column::Id.eq(id))
            .filter(Column::Attempt.eq(attempt))
            .filter(Column::Status.eq(LifecycleStatus::Running.as_str()))
            .exec(db)
            .await?;
        Ok(result.rows_affected == 1)
    }

    pub(crate) fn heartbeat(db: DatabaseConnection, id: Uuid, attempt: i32) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECONDS));
            ticker.tick().await;
            loop {
                match Self::renew(&db, id, attempt, Utc::now().fixed_offset()).await {
                    Ok(true) => {}
                    Ok(false) | Err(_) => break,
                }
                ticker.tick().await;
            }
        })
    }

    pub(crate) async fn start(db: &DatabaseConnection, id: Uuid) -> Result<Admission, DbErr> {
        Self::start_at(db, id, Utc::now().fixed_offset()).await
    }

    pub(crate) async fn start_at(
        db: &DatabaseConnection,
        id: Uuid,
        now: DateTimeWithTimeZone,
    ) -> Result<Admission, DbErr> {
        let txn = db.begin().await?;
        let initial = Entity::find_by_id(id).lock_exclusive().one(&txn).await?;
        let Some(initial) = initial else {
            txn.rollback().await?;
            return Err(DbErr::RecordNotFound("queue lifecycle".into()));
        };
        let initial_status = LifecycleStatus::from_db(&initial.status)?;
        if initial_status == LifecycleStatus::Running {
            if running_lease_is_live(initial.lease_until, now) {
                txn.rollback().await?;
                return Ok(Admission::Duplicate {
                    attempt: initial.attempt,
                });
            }
            tracing::warn!(lifecycle_id = %id, "reclaiming expired queue lease");
        }
        if matches!(
            initial_status,
            LifecycleStatus::Completed | LifecycleStatus::Cancelled
        ) {
            txn.rollback().await?;
            return Ok(Admission::Terminal);
        }
        let limit = initial.concurrency_limit;
        let key = initial.concurrency_key;
        if let Some(ref key) = key {
            crate::db::lock_for_update(&txn, &format!("queue-concurrency:{key}")).await?;
            if let Some(limit) = limit {
                let in_flight = Entity::find()
                    .filter(Column::ConcurrencyKey.eq(key.clone()))
                    .filter(Column::Status.eq(LifecycleStatus::Running.as_str()))
                    .filter(Column::LeaseUntil.is_null().or(Column::LeaseUntil.gt(now)))
                    .filter(Column::Id.ne(id))
                    .count(&txn)
                    .await?;
                if in_flight >= u64::try_from(std::cmp::Ord::max(limit, 0)).unwrap_or(0) {
                    tracing::warn!(
                        lifecycle_id = %id,
                        worker_class = %initial.worker_class,
                        concurrency_key = %key,
                        active_capacity = in_flight,
                        capacity_limit = limit,
                        "queue capacity saturated"
                    );
                    txn.rollback().await?;
                    return Ok(Admission::Saturated {
                        attempt: (initial_status == LifecycleStatus::Running)
                            .then_some(initial.attempt),
                    });
                }
            }
        }
        let recovered = initial_status == LifecycleStatus::Running;
        let lease_until = now + lease_duration();
        let result = Entity::update_many()
            .col_expr(
                Column::Status,
                Expr::value(LifecycleStatus::Running.as_str()),
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
            .col_expr(Column::LeaseUntil, Expr::value(lease_until))
            .col_expr(Column::Attempt, Expr::col(Column::Attempt).add(1))
            .filter(Column::Id.eq(id))
            .filter(if recovered {
                Column::Status.eq(LifecycleStatus::Running.as_str())
            } else {
                Column::Status.is_in([
                    LifecycleStatus::Queued.as_str(),
                    LifecycleStatus::Failed.as_str(),
                    LifecycleStatus::Retrying.as_str(),
                ])
            })
            .exec(&txn)
            .await?;
        txn.commit().await?;
        if result.rows_affected == 1 {
            let active_capacity = if let Some(ref key) = key {
                Some(
                    Entity::find()
                        .filter(Column::ConcurrencyKey.eq(key.clone()))
                        .filter(Column::Status.eq(LifecycleStatus::Running.as_str()))
                        .filter(Column::LeaseUntil.is_null().or(Column::LeaseUntil.gt(now)))
                        .count(db)
                        .await?,
                )
            } else {
                None
            };
            tracing::info!(
                lifecycle_id = %id,
                worker_class = %initial.worker_class,
                active_capacity = ?active_capacity,
                capacity_limit = ?initial.concurrency_limit,
                queue_start_seconds = std::cmp::Ord::max(now.signed_duration_since(initial.enqueue_at).num_seconds(), 0),
                "queue job admitted"
            );
        }
        Ok(if result.rows_affected == 1 {
            if recovered {
                Admission::Recovered {
                    attempt: initial.attempt + 1,
                }
            } else {
                Admission::Started {
                    attempt: initial.attempt + 1,
                }
            }
        } else {
            Admission::Duplicate {
                attempt: initial.attempt,
            }
        })
    }

    pub(crate) async fn finish(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: Option<i32>,
        status: LifecycleStatus,
        error: Option<&str>,
    ) -> Result<(), DbErr> {
        let now = Utc::now().fixed_offset();
        let mut update = Entity::update_many()
            .col_expr(Column::Status, Expr::value(status.as_str()))
            .col_expr(
                Column::LeaseUntil,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(Column::Error, Expr::value(error.map(str::to_owned)))
            .filter(Column::Id.eq(id));
        if let Some(attempt) = attempt {
            update = update
                .filter(Column::Attempt.eq(attempt))
                .filter(Column::Status.eq(LifecycleStatus::Running.as_str()));
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    Started { attempt: i32 },
    Recovered { attempt: i32 },
    Duplicate { attempt: i32 },
    Saturated { attempt: Option<i32> },
    Terminal,
}

impl Admission {
    pub(crate) fn attempt(self) -> Option<i32> {
        match self {
            Self::Started { attempt } | Self::Recovered { attempt } => Some(attempt),
            Self::Duplicate { .. } | Self::Saturated { .. } | Self::Terminal => None,
        }
    }
}

fn running_lease_is_live(
    lease_until: Option<DateTimeWithTimeZone>,
    now: DateTimeWithTimeZone,
) -> bool {
    lease_until.is_none_or(|lease| lease > now)
}

fn lease_duration() -> chrono::Duration {
    chrono::Duration::minutes(LEASE_DURATION_MINUTES)
}

pub(crate) struct Enqueue<'a> {
    pub(crate) id: Uuid,
    pub(crate) job_name: &'a str,
    pub(crate) worker_class: crate::workers::embedding_sync::WorkerClass,
    pub(crate) workspace_id: Option<Uuid>,
    pub(crate) plan: Option<&'a str>,
    pub(crate) concurrency_key: Option<&'a str>,
    pub(crate) concurrency_limit: Option<i32>,
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use migration::{Migrator, MigratorTrait};
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, EntityTrait, Statement};
    use tempfile::tempdir;

    use super::{Admission, Enqueue, Entity, LifecycleStatus, lease_duration};

    #[test]
    fn lifecycle_status_values_round_trip() {
        for status in LifecycleStatus::ALL {
            assert_eq!(LifecycleStatus::from_db(status.as_str()), Ok(status));
        }
        assert!(LifecycleStatus::from_db("unknown").is_err());
    }

    async fn database() -> sea_orm::DatabaseConnection {
        let path = tempdir().unwrap().keep().join("queue.sqlite3");
        let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .unwrap();
        Migrator::up(&db, None).await.unwrap();
        db
    }

    async fn enqueue(db: &sea_orm::DatabaseConnection, id: uuid::Uuid, limit: Option<i32>) {
        Entity::record_enqueue(
            db,
            Enqueue {
                id,
                job_name: "test",
                worker_class: crate::workers::embedding_sync::WorkerClass::Shared,
                workspace_id: None,
                plan: None,
                concurrency_key: Some("shared"),
                concurrency_limit: limit,
            },
        )
        .await
        .unwrap();
    }

    #[test]
    fn queue_start_latency_uses_recorded_timestamps() {
        let enqueue_at = Utc.timestamp_opt(1_000, 0).single().unwrap().fixed_offset();
        let start_at = Utc.timestamp_opt(1_075, 0).single().unwrap().fixed_offset();
        let row = crate::models::queue_job_lifecycles::Model {
            id: uuid::Uuid::nil(),
            provider_job_id: None,
            job_name: "job".into(),
            worker_class: "shared".into(),
            workspace_id: None,
            plan: None,
            status: "running".into(),
            enqueue_at,
            claim_at: Some(start_at),
            start_at: Some(start_at),
            retry_at: None,
            completed_at: None,
            failed_at: None,
            cancelled_at: None,
            admitted_at: Some(start_at),
            lease_until: Some(start_at + chrono::Duration::minutes(5)),
            attempt: 1,
            concurrency_key: None,
            concurrency_limit: None,
            error: None,
            created_at: enqueue_at,
            updated_at: start_at,
        };
        assert_eq!(
            row.start_at
                .map(|start| (start - row.enqueue_at).num_seconds()),
            Some(75)
        );
    }

    #[test]
    fn live_lease_is_duplicate_but_expired_lease_is_recoverable() {
        let now = Utc.timestamp_opt(2_000, 0).single().unwrap().fixed_offset();
        assert!(super::running_lease_is_live(
            Some(now + chrono::Duration::seconds(1)),
            now
        ));
        assert!(!super::running_lease_is_live(
            Some(now - chrono::Duration::seconds(1)),
            now
        ));
        assert!(super::running_lease_is_live(None, now));
    }

    #[tokio::test]
    async fn admission_is_attempt_fenced_and_heartbeat_is_deterministic() {
        let db = database().await;
        let id = uuid::Uuid::now_v7();
        enqueue(&db, id, None).await;
        let now = Utc.timestamp_opt(3_000, 0).single().unwrap().fixed_offset();
        assert_eq!(
            Entity::start_at(&db, id, now).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        assert!(matches!(
            Entity::start_at(&db, id, now).await.unwrap(),
            Admission::Duplicate { .. }
        ));
        assert!(Entity::renew_at(&db, id, 1, now).await.unwrap());
        let row = Entity::find_by_id(id).one(&db).await.unwrap().unwrap();
        assert_eq!(row.lease_until, Some(now + lease_duration()));
        assert!(!Entity::renew_at(&db, id, 2, now).await.unwrap());
    }

    #[tokio::test]
    async fn expired_attempt_recovers_and_stale_completion_is_rejected() {
        let db = database().await;
        let id = uuid::Uuid::now_v7();
        enqueue(&db, id, None).await;
        let first = Utc.timestamp_opt(4_000, 0).single().unwrap().fixed_offset();
        assert_eq!(
            Entity::start_at(&db, id, first).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        let second = first + lease_duration() + chrono::Duration::seconds(1);
        assert_eq!(
            Entity::start_at(&db, id, second).await.unwrap(),
            Admission::Recovered { attempt: 2 }
        );
        assert!(
            Entity::finish(
                &db,
                id,
                Some(1),
                crate::models::queue_job_lifecycles::LifecycleStatus::Completed,
                None
            )
            .await
            .is_err()
        );
        assert!(
            Entity::finish(
                &db,
                id,
                Some(2),
                crate::models::queue_job_lifecycles::LifecycleStatus::Completed,
                None
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn concurrent_admission_allows_one_attempt() {
        let directory = tempdir().unwrap();
        let uri = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("queue.sqlite3").display()
        );
        let first = Database::connect(&uri).await.unwrap();
        let second = Database::connect(&uri).await.unwrap();
        Migrator::up(&first, None).await.unwrap();
        for db in [&first, &second] {
            db.execute_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;",
            ))
            .await
            .unwrap();
        }
        let id = uuid::Uuid::now_v7();
        enqueue(&first, id, None).await;
        let now = Utc.timestamp_opt(5_000, 0).single().unwrap().fixed_offset();
        let (left, right) = tokio::join!(
            Entity::start_at(&first, id, now),
            Entity::start_at(&second, id, now)
        );
        let admissions = [left.unwrap(), right.unwrap()];
        assert_eq!(
            admissions.iter().filter(|a| a.attempt().is_some()).count(),
            1
        );
    }

    #[tokio::test]
    async fn saturation_defers_without_consuming_or_duplicating_the_retry() {
        let db = database().await;
        let active = uuid::Uuid::now_v7();
        let waiting = uuid::Uuid::now_v7();
        enqueue(&db, active, Some(1)).await;
        enqueue(&db, waiting, Some(1)).await;
        let now = Utc.timestamp_opt(6_000, 0).single().unwrap().fixed_offset();
        assert!(
            Entity::start_at(&db, active, now)
                .await
                .unwrap()
                .attempt()
                .is_some()
        );
        assert_eq!(
            Entity::start_at(&db, waiting, now).await.unwrap(),
            Admission::Saturated { attempt: None }
        );
        Entity::defer(&db, waiting, None, "capacity").await.unwrap();
        let row = Entity::find_by_id(waiting).one(&db).await.unwrap().unwrap();
        assert_eq!(row.status, "retrying");
        assert!(row.lease_until.is_none());
        assert_eq!(
            Entity::start_at(&db, waiting, now).await.unwrap(),
            Admission::Saturated { attempt: None }
        );
        let row = Entity::find_by_id(waiting).one(&db).await.unwrap().unwrap();
        assert_eq!(row.status, "retrying");
        assert_eq!(row.attempt, 0);
    }

    #[tokio::test]
    async fn expired_saturation_defers_with_its_attempt_and_recovers_later() {
        let db = database().await;
        let active = uuid::Uuid::now_v7();
        let expired = uuid::Uuid::now_v7();
        enqueue(&db, active, Some(1)).await;
        enqueue(&db, expired, Some(1)).await;
        let first = Utc.timestamp_opt(7_000, 0).single().unwrap().fixed_offset();
        assert_eq!(
            Entity::start_at(&db, expired, first).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        let expired_at = first + lease_duration() + chrono::Duration::seconds(1);
        assert_eq!(
            Entity::start_at(&db, active, expired_at).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        assert_eq!(
            Entity::start_at(&db, expired, expired_at).await.unwrap(),
            Admission::Saturated { attempt: Some(1) }
        );
        Entity::defer_at(&db, expired, 1, "capacity", expired_at)
            .await
            .unwrap();
        let row = Entity::find_by_id(expired).one(&db).await.unwrap().unwrap();
        assert_eq!(row.status, "retrying");
        assert_eq!(row.attempt, 1);
        assert!(
            Entity::finish(
                &db,
                expired,
                Some(1),
                crate::models::queue_job_lifecycles::LifecycleStatus::Completed,
                None
            )
            .await
            .is_err()
        );
        Entity::finish(
            &db,
            active,
            Some(1),
            crate::models::queue_job_lifecycles::LifecycleStatus::Completed,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            Entity::start_at(&db, expired, expired_at).await.unwrap(),
            Admission::Started { attempt: 2 }
        );
    }

    #[tokio::test]
    async fn renewed_expired_saturation_cannot_be_deferred() {
        let db = database().await;
        let active = uuid::Uuid::now_v7();
        let expired = uuid::Uuid::now_v7();
        enqueue(&db, active, Some(1)).await;
        enqueue(&db, expired, Some(1)).await;
        let first = Utc.timestamp_opt(8_000, 0).single().unwrap().fixed_offset();
        assert_eq!(
            Entity::start_at(&db, expired, first).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        let observed = first + lease_duration() + chrono::Duration::seconds(1);
        assert_eq!(
            Entity::start_at(&db, active, observed).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        assert_eq!(
            Entity::start_at(&db, expired, observed).await.unwrap(),
            Admission::Saturated { attempt: Some(1) }
        );
        let renewed = observed + chrono::Duration::seconds(1);
        assert!(Entity::renew_at(&db, expired, 1, renewed).await.unwrap());
        assert!(
            Entity::defer_at(&db, expired, 1, "capacity", observed)
                .await
                .is_err()
        );
        let row = Entity::find_by_id(expired).one(&db).await.unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert_eq!(row.attempt, 1);
        assert!(row.lease_until.unwrap() > observed);
        assert!(matches!(
            Entity::start_at(&db, expired, renewed).await.unwrap(),
            Admission::Duplicate { attempt: 1 }
        ));
    }

    #[tokio::test]
    async fn live_attempt_failure_defers_without_changing_attempt() {
        let db = database().await;
        let id = uuid::Uuid::now_v7();
        enqueue(&db, id, None).await;
        let now = Utc.timestamp_opt(9_000, 0).single().unwrap().fixed_offset();
        assert_eq!(
            Entity::start_at(&db, id, now).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        Entity::defer(&db, id, Some(1), "worker failed")
            .await
            .unwrap();
        let row = Entity::find_by_id(id).one(&db).await.unwrap().unwrap();
        assert_eq!(row.status, "retrying");
        assert_eq!(row.attempt, 1);
        assert!(Entity::defer(&db, id, Some(0), "stale").await.is_err());
        assert_eq!(
            Entity::find_by_id(id)
                .one(&db)
                .await
                .unwrap()
                .unwrap()
                .status,
            "retrying"
        );
    }
}
