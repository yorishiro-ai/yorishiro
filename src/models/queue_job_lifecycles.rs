//! The job lifecycle table: enqueue, dispatch, lease heartbeat, deferral and completion state for every background job.

use crate::error::OptionExt;
pub use crate::models::_entities::queue_job_lifecycles::{ActiveModel, Column, Entity, Model};
use sea_orm::ActiveValue::Set;
use sea_orm::entity::prelude::*;
use sea_orm::{DatabaseTransaction, ExprTrait, PaginatorTrait, QuerySelect};
use tokio::task::JoinHandle;

use crate::models::db_enum::db_enum;

const HEARTBEAT_INTERVAL_SECONDS: u64 = 60;
const LEASE_DURATION_MINUTES: i64 = 5;

#[derive(serde::Serialize, schemars::JsonSchema)]
pub(crate) struct BulkJobStatus {
    job_name: String,
    status: LifecycleStatus,
    result: Option<serde_json::Value>,
    /// The client-facing failure reason.
    error: Option<&'static str>,
}

pub(crate) async fn migration_job_status(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<BulkJobStatus, crate::YorishiroError> {
    bulk_job_status(
        db,
        workspace_id,
        id,
        &["undo", "fill_defaults", "reindex", "startup_reindex"],
    )
    .await
}

/// Opens the caller's migration session and reads a migration job's status.
///
/// # Errors
/// Returns the session, authorization or lookup error.
pub(crate) async fn migration_job_status_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::access_keys::AuthContext,
    id: Uuid,
) -> Result<BulkJobStatus, crate::YorishiroError> {
    crate::models::iam::session::WorkspaceSession::read(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Job(id),
        crate::models::iam::policy::Action::GetMigrationJob,
        async |session| migration_job_status(session.txn(), session.workspace_id()?, id).await,
    )
    .await
}

pub(crate) async fn bulk_job_status(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
    allowed_jobs: &[&str],
) -> Result<BulkJobStatus, crate::YorishiroError> {
    use crate::error::ResultExt;
    use crate::models::job_results::{self, JobKind};
    let row = Entity::find_by_id(id)
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::JobName.is_in(allowed_jobs.iter().copied()))
        .one(db)
        .await
        .internal()?
        .or_not_found("migration job not found")?;
    let action = if row.job_name == "undo" {
        JobKind::UndoMigrationJob
    } else if row.job_name == "fill_defaults" {
        JobKind::FillDefaults
    } else {
        JobKind::BackgroundJobCompleted
    };
    let status = LifecycleStatus::from_row(&row.status).internal()?;
    Ok(BulkJobStatus {
        job_name: row.job_name,
        status,
        result: job_results::job_result(db, workspace_id, id, action).await?,
        error: (status == LifecycleStatus::Failed).then(|| public_job_error(row.error.as_deref())),
    })
}

fn public_job_error(error: Option<&str>) -> &'static str {
    // The stored diagnostics can contain database or provider details.
    match error {
        Some(error) if error.starts_with("conflict: ") => {
            "The workspace changed while the job was running. Retry the job."
        }
        Some("unauthenticated") => "The job's access key is no longer valid.",
        Some(error) if error.starts_with("access denied: ") => {
            "The job no longer has the required permission."
        }
        Some(
            "backend unavailable: no embedding provider is configured"
            | "embedding provider absent"
            | "embedding provider disabled",
        ) => "No embedding provider is configured.",
        Some(error)
            if error.starts_with("backend unavailable: reindex could not prepare embeddings")
                || error.starts_with("reindex incomplete:") =>
        {
            "The embedding provider could not prepare all workspace embeddings."
        }
        _ => "The job failed. See the worker logs for details.",
    }
}

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
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub fn from_row(value: &str) -> Result<Self, DbErr> {
        Self::from_db_str(value)
            .ok_or_else(|| DbErr::Type(format!("unknown queue lifecycle status {value:?}")))
    }
}

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::generated_id(this.id);
        this.created_at = crate::db::stamped_created_at(insert, this.created_at);
        this.updated_at = crate::db::stamped_updated_at(this.updated_at);
        Ok(this)
    }
}

impl Entity {
    /// Inserts a `queued` lifecycle row for a new job.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
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
            enqueue_at: Set(crate::data::time::database_instant(crate::data::time::now())),
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
            request_id: Set(crate::controllers::middleware::access_log::current_request_id()),
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
        let result = Self::update_many()
            .col_expr(
                Column::ProviderJobId,
                Expr::value(Some(provider_job_id.to_owned())),
            )
            .col_expr(Column::UpdatedAt, Expr::value(crate::data::time::now()))
            .filter(Column::Id.eq(id))
            .exec(db)
            .await?;
        if result.rows_affected == 1 {
            Ok(())
        } else {
            Err(DbErr::RecordNotFound("queue lifecycle".into()))
        }
    }

    /// Whether a workspace already has queued or running full embedding repair work.
    ///
    /// # Errors
    /// Returns an error if the lifecycle lookup fails.
    pub async fn has_active_reindex(
        db: &impl ConnectionTrait,
        workspace_id: Uuid,
    ) -> Result<bool, DbErr> {
        use crate::workers::reindex::ReindexArgs;

        Ok(Self::find()
            .filter(Column::JobName.is_in([ReindexArgs::JOB_NAME, ReindexArgs::STARTUP_JOB_NAME]))
            .filter(Column::WorkspaceId.eq(workspace_id))
            .filter(Column::Status.is_in([
                LifecycleStatus::Queued.as_db_str(),
                LifecycleStatus::Running.as_db_str(),
                LifecycleStatus::Retrying.as_db_str(),
            ]))
            .count(db)
            .await?
            > 0)
    }

    /// Whether a scheduled reindex of the workspace failed since `since`.
    ///
    /// # Errors
    /// Returns an error if the lifecycle lookup fails.
    pub async fn startup_reindex_failed_since(
        db: &impl ConnectionTrait,
        workspace_id: Uuid,
        since: DateTimeWithTimeZone,
    ) -> Result<bool, DbErr> {
        Ok(Self::find()
            .filter(Column::JobName.eq(crate::workers::reindex::ReindexArgs::STARTUP_JOB_NAME))
            .filter(Column::WorkspaceId.eq(workspace_id))
            .filter(Column::Status.eq(LifecycleStatus::Failed.as_db_str()))
            .filter(Column::FailedAt.gt(since))
            .count(db)
            .await?
            > 0)
    }

    /// Puts a queued, retrying or running attempt back to `retrying`, due now.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn defer(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: Option<i32>,
        error: &str,
    ) -> Result<(), DbErr> {
        let now = crate::data::time::database_instant(crate::data::time::now());
        let result = Self::update_many()
            .col_expr(
                Column::Status,
                Expr::value(LifecycleStatus::Retrying.as_db_str()),
            )
            .col_expr(Column::RetryAt, Expr::value(now))
            .col_expr(Column::UpdatedAt, Expr::value(now))
            .col_expr(Column::ProviderJobId, Expr::value(None::<String>))
            .col_expr(
                Column::LeaseUntil,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(Column::Error, Expr::value(Some(error.to_owned())))
            .filter(Column::Id.eq(id))
            .filter(attempt.map_or_else(
                || {
                    Column::Status.is_in([
                        LifecycleStatus::Queued.as_db_str(),
                        LifecycleStatus::Retrying.as_db_str(),
                    ])
                },
                |attempt| {
                    Column::Attempt
                        .eq(attempt)
                        .and(Column::Status.eq(LifecycleStatus::Running.as_db_str()))
                },
            ))
            .exec(db)
            .await?;
        tracing::warn!(lifecycle_id = %id, error = error, "queue job deferred for retry");
        if result.rows_affected == 0 {
            return Err(DbErr::RecordNotFound("active queue lease".into()));
        }
        Ok(())
    }

    /// Defers a running attempt whose lease has lapsed at `now`.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn defer_at(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: i32,
        error: &str,
        now: DateTimeWithTimeZone,
    ) -> Result<(), DbErr> {
        let result = Self::update_many()
            .col_expr(
                Column::Status,
                Expr::value(LifecycleStatus::Retrying.as_db_str()),
            )
            .col_expr(Column::RetryAt, Expr::value(now))
            .col_expr(Column::UpdatedAt, Expr::value(now))
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
        tracing::warn!(lifecycle_id = %id, error = error, "queue job deferred for retry");
        if result.rows_affected == 0 {
            return Err(DbErr::RecordNotFound("active queue lease".into()));
        }
        Ok(())
    }

    /// Extends the lease of a running attempt and returns whether the attempt still held it.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn renew_at(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: i32,
        now: DateTimeWithTimeZone,
    ) -> Result<bool, DbErr> {
        let result = Self::update_many()
            .col_expr(Column::LeaseUntil, Expr::value(now + lease_duration()))
            .col_expr(Column::UpdatedAt, Expr::value(now))
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
                match Self::renew_at(
                    &db,
                    id,
                    attempt,
                    crate::data::time::database_instant(crate::data::time::now()),
                )
                .await
                {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        tracing::warn!(lifecycle_id = %id, error = %error, "queue lease renewal failed");
                        break;
                    }
                }
                ticker.tick().await;
            }
        }))
    }

    pub(crate) async fn start(db: &DatabaseConnection, id: Uuid) -> Result<Admission, DbErr> {
        Self::start_at(
            db,
            id,
            crate::data::time::database_instant(crate::data::time::now()),
        )
        .await
    }

    /// Admits one delivery of a lifecycle at `now`, committing only when it starts or recovers an attempt.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn start_at(
        db: &DatabaseConnection,
        id: Uuid,
        now: DateTimeWithTimeZone,
    ) -> Result<Admission, DbErr> {
        let txn = crate::db::begin_write(db).await?;
        let admission = admit(&txn, id, now).await?;
        match admission {
            // A terminal admission may have just failed a job that used every attempt.
            Admission::Started { .. } | Admission::Recovered { .. } | Admission::Terminal => {
                txn.commit().await?;
            }
            _ => txn.rollback().await?,
        }
        if let Admission::Started { attempt } | Admission::Recovered { attempt } = admission {
            Self::record_attempt(db, id, attempt).await;
        }
        Ok(admission)
    }

    /// Moves a lifecycle to `status`, stamping the matching timestamp and clearing the lease.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn finish(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: Option<i32>,
        status: LifecycleStatus,
        error: Option<&str>,
    ) -> Result<(), DbErr> {
        Self::finish_with_trail(db, id, attempt, status, error, TrailNote::default()).await
    }

    /// Like [`Self::finish`], and names in the trail row what the attempt touched and why it failed.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn finish_with_trail(
        db: &impl ConnectionTrait,
        id: Uuid,
        attempt: Option<i32>,
        status: LifecycleStatus,
        error: Option<&str>,
        note: TrailNote,
    ) -> Result<(), DbErr> {
        let now = crate::data::time::now();
        let mut update = Self::update_many()
            .col_expr(Column::Status, Expr::value(status.as_db_str()))
            .col_expr(Column::UpdatedAt, Expr::value(now))
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
        if matches!(
            status,
            LifecycleStatus::Completed
                | LifecycleStatus::Failed
                | LifecycleStatus::Cancelled
                | LifecycleStatus::Unavailable
        ) {
            Self::record_outcome(db, id, status, note).await;
        }
        Ok(())
    }

    /// The request that enqueued the job, when it was enqueued by one.
    ///
    /// # Errors
    /// Returns an error if the lookup fails.
    pub async fn request_id(db: &impl ConnectionTrait, id: Uuid) -> Result<Option<String>, DbErr> {
        Ok(Self::find_by_id(id)
            .one(db)
            .await?
            .and_then(|job| job.request_id))
    }

    /// Appends a row that marks one attempt starting, so retries and reclaimed leases show in the trail.
    async fn record_attempt(db: &impl ConnectionTrait, id: Uuid, attempt: i32) {
        let recorded = match Self::find_by_id(id).one(db).await {
            Ok(Some(job)) => crate::models::api_events::Call::job(
                job.request_id.unwrap_or_else(|| "req-id-none".to_owned()),
                id,
                job.job_name,
                job.workspace_id,
                None,
                attempt,
            )
            .append(db)
            .await
            .map_err(|error| error.to_string()),
            Ok(None) => Err("the job row is missing".to_owned()),
            Err(error) => Err(error.to_string()),
        };
        if let Err(error) = recorded {
            tracing::error!(lifecycle_id = %id, %error, "the job start could not be recorded in the trail");
        }
    }

    /// Appends the job's end to the trail under the request that started it, so a call can be followed to its final result.
    async fn record_outcome(
        db: &impl ConnectionTrait,
        id: Uuid,
        status: LifecycleStatus,
        note: TrailNote,
    ) {
        let outcome = match Self::find_by_id(id).one(db).await {
            Ok(Some(job)) => crate::models::api_events::Call::job(
                job.request_id.unwrap_or_else(|| "req-id-none".to_owned()),
                id,
                job.job_name,
                job.workspace_id,
                Some(match status {
                    LifecycleStatus::Completed => "success",
                    LifecycleStatus::Cancelled => "cancelled",
                    LifecycleStatus::Unavailable => "unavailable",
                    _ => "failed",
                }),
                job.attempt,
            )
            .with_resources(serde_json::Value::Array(note.resources))
            .with_error(note.error)
            .append(db)
            .await
            .map_err(|error| error.to_string()),
            Ok(None) => Err("the job row is missing".to_owned()),
            Err(error) => Err(error.to_string()),
        };
        if let Err(error) = outcome {
            tracing::error!(lifecycle_id = %id, %error, "the job outcome could not be recorded in the trail");
        }
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
    if row.attempt >= MAX_ATTEMPTS {
        // The job has used every attempt: it stays failed instead of being claimed again.
        Entity::update_many()
            .col_expr(
                Column::Status,
                Expr::value(LifecycleStatus::Failed.as_db_str()),
            )
            .col_expr(Column::FailedAt, Expr::value(now))
            .col_expr(Column::UpdatedAt, Expr::value(now))
            .col_expr(Column::Error, Expr::value("attempt limit reached"))
            .col_expr(
                Column::LeaseUntil,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .filter(Column::Id.eq(id))
            .filter(Column::Status.ne(LifecycleStatus::Failed.as_db_str()))
            .exec(txn)
            .await?;
        return Ok(Admission::Terminal);
    }
    let recovered = status == LifecycleStatus::Running;

    let in_flight = match (&row.concurrency_key, row.concurrency_limit) {
        (Some(key), Some(_)) => {
            crate::db::lock_for_update(txn, &format!("queue-concurrency:{key}")).await?;
            Some(in_flight(txn, key, now, id).await?)
        }
        _ => None,
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
        queue_start_seconds = Ord::max(now.signed_duration_since(row.enqueue_at).num_seconds(), 0),
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

/// What an attempt adds to its trail row: what it touched and why it failed.
#[derive(Default)]
pub struct TrailNote {
    pub resources: Vec<serde_json::Value>,
    /// The error code and a description that never repeats input.
    pub error: Option<(String, String)>,
}

/// The attempts a job may start before it stays failed.
pub const MAX_ATTEMPTS: i32 = 5;

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
        .col_expr(Column::UpdatedAt, Expr::value(now))
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
/// Dropping it stops the renewal task, so a cancelled or panicking worker cannot keep extending the lease of a job nobody runs.
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

#[must_use]
pub fn running_lease_is_live(
    lease_until: Option<DateTimeWithTimeZone>,
    now: DateTimeWithTimeZone,
) -> bool {
    lease_until.is_none_or(|lease| lease > now)
}

#[must_use]
pub const fn lease_duration() -> chrono::Duration {
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

/// Opens the caller's read session and reads an import job's status.
///
/// # Errors
/// Returns the session, authorization or lookup error.
pub(crate) async fn import_job_status_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::access_keys::AuthContext,
    id: Uuid,
) -> Result<BulkJobStatus, crate::YorishiroError> {
    crate::models::iam::session::WorkspaceSession::read(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Job(id),
        crate::models::iam::policy::Action::GetImportJob,
        async |session| {
            bulk_job_status(session.txn(), session.workspace_id()?, id, &["import"]).await
        },
    )
    .await
}
