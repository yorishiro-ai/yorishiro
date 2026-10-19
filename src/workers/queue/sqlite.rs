use std::{collections::HashMap, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use loco_rs::prelude::async_trait;
use loco_rs::{
    bgworker::{Job, JobHandler, JobId, JobStatus, QueueProvider},
    config::SqliteQueueConfig,
};
use sea_orm::sqlx::{
    self, ConnectOptions, Row,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow},
};
use sea_orm::{
    ConnectionTrait, ExprTrait,
    sea_query::{Alias, Condition, Expr, Query},
};
use serde_json::Value as JsonValue;

type SqlitePool = sqlx::SqlitePool;

#[derive(Clone)]
pub struct SqliteTimezoneQueue {
    pub pool: SqlitePool,
    handlers: Arc<tokio::sync::Mutex<HashMap<String, Arc<JobHandler>>>>,
    num_workers: u32,
    poll_seconds: u32,
    reaper: Option<loco_rs::config::ReaperConfig>,
    token: tokio::sync::watch::Sender<bool>,
    pub timezone: crate::time::Timezone,
}

#[must_use]
pub fn exact_parts(at: DateTime<Utc>) -> (i64, i32) {
    (at.timestamp(), at.timestamp_subsec_nanos() as i32)
}

impl SqliteTimezoneQueue {
    /// Creates a provider using the configured SQLite queue settings.
    ///
    /// # Errors
    /// Returns an error when the pool cannot be configured or initialized.
    pub async fn connect(
        config: &SqliteQueueConfig,
        timezone: crate::time::Timezone,
    ) -> loco_rs::Result<Self> {
        let mut options: SqliteConnectOptions = config.uri.parse()?;
        options = options.busy_timeout(Duration::from_secs(5));
        if !config.enable_logging {
            options = options.disable_statement_logging();
        }
        let pool = SqlitePoolOptions::new()
            .min_connections(config.min_connections)
            .max_connections(config.max_connections)
            .idle_timeout(Duration::from_millis(config.idle_timeout))
            .acquire_timeout(Duration::from_millis(config.connect_timeout))
            .connect_with(options)
            .await?;
        initialize_schema(&pool, timezone).await?;
        ensure_exact_columns(&pool).await?;
        ensure_queue_indexes(&pool).await?;
        Ok(Self {
            pool,
            handlers: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            num_workers: config.num_workers,
            poll_seconds: config.poll_interval_sec,
            reaper: config.reaper.clone(),
            token: tokio::sync::watch::channel(false).0,
            timezone,
        })
    }
}

fn parse_storage(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|at| at.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            [
                "%Y-%m-%d %H:%M:%S%.f",
                "%Y-%m-%dT%H:%M:%S%.f",
                "%Y-%m-%d %H:%M:%S",
                "%Y-%m-%dT%H:%M:%S",
            ]
            .into_iter()
            .find_map(|format| chrono::NaiveDateTime::parse_from_str(value, format).ok())
            .map(|at| at.and_utc())
        })
}

#[async_trait]
impl super::runtime::Backend for SqliteTimezoneQueue {
    async fn claim(
        &self,
        tags: &[String],
        _stop: &mut tokio::sync::watch::Receiver<bool>,
    ) -> loco_rs::Result<Option<super::runtime::Delivery>> {
        Ok(dequeue(&self.pool, tags, self.timezone)
            .await?
            .map(|job| super::runtime::Delivery { job, queue: None }))
    }

    async fn complete(&self, delivery: &super::runtime::Delivery) -> loco_rs::Result<()> {
        complete(
            &self.pool,
            &delivery.job.id,
            delivery.job.interval,
            self.timezone,
        )
        .await
    }

    async fn fail(
        &self,
        delivery: &super::runtime::Delivery,
        error: &loco_rs::Error,
    ) -> loco_rs::Result<()> {
        fail(&self.pool, &delivery.job.id, error, self.timezone).await
    }
}

#[async_trait]
impl QueueProvider for SqliteTimezoneQueue {
    async fn enqueue(
        &self,
        class: String,
        _queue: Option<String>,
        args: JsonValue,
        tags: Option<Vec<String>>,
        priority: Option<i32>,
    ) -> loco_rs::Result<Option<String>> {
        let now = crate::time::queue_now(self.timezone).with_timezone(&Utc);
        let (sec, nanos) = exact_parts(now);
        let now_local = crate::time::queue_storage(now, self.timezone);
        let id = ulid::Ulid::generate().to_string();
        let tags = tags.map(serde_json::to_value).transpose()?;
        let statement = Query::insert()
            .into_table(Alias::new("sqlt_loco_queue"))
            .columns(
                [
                    "id",
                    "task_data",
                    "name",
                    "run_at",
                    "run_at_epoch_seconds",
                    "run_at_epoch_nanos",
                    "interval",
                    "tags",
                    "priority",
                    "created_at",
                    "created_at_epoch_seconds",
                    "created_at_epoch_nanos",
                    "created_at_legacy_checked",
                    "updated_at",
                    "updated_at_epoch_seconds",
                    "updated_at_epoch_nanos",
                    "updated_at_legacy_checked",
                ]
                .map(Alias::new),
            )
            .values_panic([
                id.clone().into(),
                args.into(),
                class.into(),
                now_local.clone().into(),
                sec.into(),
                nanos.into(),
                None::<i64>.into(),
                tags.into(),
                priority.unwrap_or(0).into(),
                now_local.clone().into(),
                sec.into(),
                nanos.into(),
                1.into(),
                now_local.into(),
                sec.into(),
                nanos.into(),
                1.into(),
            ])
            .to_owned();
        let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(self.pool.clone());
        db.execute_raw(db.get_database_backend().build(&statement))
            .await?;
        Ok(Some(id))
    }

    async fn enqueue_batch(
        &self,
        class: String,
        _queue: Option<String>,
        jobs: Vec<(JsonValue, Option<i32>)>,
        tags: Option<Vec<String>>,
    ) -> loco_rs::Result<Vec<JobId>> {
        if jobs.is_empty() {
            return Ok(Vec::new());
        }
        let now = crate::time::queue_now(self.timezone).with_timezone(&Utc);
        let (sec, nanos) = exact_parts(now);
        let run_at = crate::time::queue_storage(now, self.timezone);
        let now_local = run_at.clone();
        let tags = tags.map(serde_json::to_value).transpose()?;
        let mut ids = Vec::with_capacity(jobs.len());
        let mut unique_ids = std::collections::HashSet::with_capacity(jobs.len());
        for _ in &jobs {
            let id = ulid::Ulid::generate().to_string();
            if !unique_ids.insert(id.clone()) {
                return Err(loco_rs::Error::Message(
                    "duplicate ULID generated within SQLite batch".into(),
                ));
            }
            ids.push(id);
        }
        let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(self.pool.clone());
        let tx = crate::db::begin_write(&db).await?;
        let mut id_iter = ids.iter();
        for chunk in jobs.chunks(1000) {
            let mut query = Query::insert()
                .into_table(Alias::new("sqlt_loco_queue"))
                .columns(
                    [
                        "id",
                        "task_data",
                        "name",
                        "run_at",
                        "run_at_epoch_seconds",
                        "run_at_epoch_nanos",
                        "interval",
                        "tags",
                        "priority",
                        "created_at",
                        "created_at_epoch_seconds",
                        "created_at_epoch_nanos",
                        "created_at_legacy_checked",
                        "updated_at",
                        "updated_at_epoch_seconds",
                        "updated_at_epoch_nanos",
                        "updated_at_legacy_checked",
                    ]
                    .map(Alias::new),
                )
                .to_owned();
            for (data, priority) in chunk {
                let id = id_iter.next().expect("one id per batch entry").clone();
                query.values_panic([
                    id.into(),
                    data.clone().into(),
                    class.clone().into(),
                    run_at.clone().into(),
                    sec.into(),
                    nanos.into(),
                    None::<i64>.into(),
                    tags.clone().into(),
                    priority.unwrap_or(0).into(),
                    now_local.clone().into(),
                    sec.into(),
                    nanos.into(),
                    1.into(),
                    now_local.clone().into(),
                    sec.into(),
                    nanos.into(),
                    1.into(),
                ]);
            }
            tx.execute_raw(tx.get_database_backend().build(&query))
                .await?;
        }
        tx.commit().await?;
        Ok(ids)
    }

    async fn register_handler(&self, name: String, handler: JobHandler) -> loco_rs::Result<()> {
        self.handlers.lock().await.insert(name, Arc::new(handler));
        Ok(())
    }
    async fn run(&self, tags: Vec<String>) -> loco_rs::Result<()> {
        super::runtime::run(
            self.clone(),
            self.handlers.clone(),
            self.num_workers,
            Duration::from_secs(self.poll_seconds.into()),
            self.reaper.clone(),
            self.token.clone(),
            tags,
        )
        .await
    }
    async fn setup(&self) -> loco_rs::Result<()> {
        initialize_schema(&self.pool, self.timezone).await?;
        ensure_exact_columns(&self.pool).await?;
        ensure_queue_indexes(&self.pool).await
    }
    async fn clear(&self) -> loco_rs::Result<()> {
        loco_rs::bgworker::sqlt::clear(&self.pool).await
    }
    async fn ping(&self) -> loco_rs::Result<()> {
        loco_rs::bgworker::sqlt::ping(&self.pool).await?;
        Ok(())
    }
    async fn get_jobs(
        &self,
        status: Option<&Vec<JobStatus>>,
        age_days: Option<i64>,
    ) -> loco_rs::Result<Vec<Job>> {
        get_jobs(&self.pool, status, age_days, self.timezone).await
    }
    async fn cancel_jobs_by_name(&self, name: &str) -> loco_rs::Result<()> {
        let now = crate::time::queue_now(self.timezone).with_timezone(&Utc);
        let (sec, nanos) = exact_parts(now);
        let statement = Query::update()
            .table(Alias::new("sqlt_loco_queue"))
            .values([
                (
                    Alias::new("status"),
                    Expr::val(JobStatus::Cancelled.to_string()),
                ),
                (
                    Alias::new("updated_at"),
                    Expr::val(crate::time::queue_storage(now, self.timezone)),
                ),
                (Alias::new("updated_at_epoch_seconds"), Expr::val(sec)),
                (Alias::new("updated_at_epoch_nanos"), Expr::val(nanos)),
                (Alias::new("updated_at_legacy_checked"), Expr::val(1)),
            ])
            .and_where(Expr::col(Alias::new("name")).eq(name))
            .and_where(Expr::col(Alias::new("status")).eq(JobStatus::Queued.to_string()))
            .to_owned();
        let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(self.pool.clone());
        db.execute_raw(db.get_database_backend().build(&statement))
            .await?;
        Ok(())
    }
    async fn clear_by_status(&self, status: Vec<JobStatus>) -> loco_rs::Result<()> {
        if status.is_empty() {
            return Ok(());
        }
        loco_rs::bgworker::sqlt::clear_by_status(&self.pool, status).await
    }
    async fn clear_jobs_older_than(
        &self,
        age_days: i64,
        status: Option<&Vec<JobStatus>>,
    ) -> loco_rs::Result<()> {
        let now = crate::time::queue_now(self.timezone).with_timezone(&Utc);
        let (cutoff_sec, cutoff_nanos) = exact_parts(now - chrono::Duration::days(age_days));
        backfill_lifecycle_epochs(&self.pool).await?;
        clear_jobs_through(&self.pool, cutoff_sec, cutoff_nanos, status).await?;
        Ok(())
    }
    async fn requeue(&self, age_minutes: &i64) -> loco_rs::Result<()> {
        let now = crate::time::queue_now(self.timezone).with_timezone(&Utc);
        let (cutoff_sec, cutoff_nanos) = exact_parts(now - chrono::Duration::minutes(*age_minutes));
        let (now_sec, now_nanos) = exact_parts(now);
        let updated_at = crate::time::queue_storage(now, self.timezone);
        backfill_lifecycle_epochs(&self.pool).await?;
        requeue_through(
            &self.pool,
            cutoff_sec,
            cutoff_nanos,
            updated_at,
            now_sec,
            now_nanos,
        )
        .await?;
        Ok(())
    }
    async fn retry_failed(&self, id: Option<&str>) -> loco_rs::Result<u64> {
        let now = crate::time::queue_now(self.timezone).with_timezone(&Utc);
        let (sec, nanos) = exact_parts(now);
        let local = crate::time::queue_storage(now, self.timezone);
        let updated_at = crate::time::queue_storage(now, self.timezone);
        let mut statement = Query::update()
            .table(Alias::new("sqlt_loco_queue"))
            .values([
                (
                    Alias::new("status"),
                    Expr::val(JobStatus::Queued.to_string()),
                ),
                (Alias::new("updated_at"), Expr::val(updated_at)),
                (Alias::new("updated_at_epoch_seconds"), Expr::val(sec)),
                (Alias::new("updated_at_epoch_nanos"), Expr::val(nanos)),
                (Alias::new("updated_at_legacy_checked"), Expr::val(1)),
                (Alias::new("run_at"), Expr::val(local)),
                (Alias::new("run_at_epoch_seconds"), Expr::val(sec)),
                (Alias::new("run_at_epoch_nanos"), Expr::val(nanos)),
            ])
            .and_where(Expr::col(Alias::new("status")).eq(JobStatus::Failed.to_string()))
            .to_owned();
        if let Some(id) = id {
            statement.and_where(Expr::col(Alias::new("id")).eq(id));
        }
        let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(self.pool.clone());
        Ok(db
            .execute_raw(db.get_database_backend().build(&statement))
            .await?
            .rows_affected())
    }

    fn describe(&self) -> String {
        "sqlite queue".into()
    }
    fn shutdown(&self) -> loco_rs::Result<()> {
        let _ = self.token.send(true);
        Ok(())
    }
}

/// Deletes queue jobs at or before the supplied exact instant.
///
/// # Errors
/// Returns an error if the cleanup query fails.
pub async fn clear_jobs_through(
    pool: &SqlitePool,
    cutoff_sec: i64,
    cutoff_nanos: i32,
    status: Option<&Vec<JobStatus>>,
) -> loco_rs::Result<()> {
    let mut statement = Query::delete()
        .from_table(Alias::new("sqlt_loco_queue"))
        .cond_where(
            Condition::any()
                .add(Expr::col(Alias::new("created_at_epoch_seconds")).lt(cutoff_sec))
                .add(
                    Condition::all()
                        .add(Expr::col(Alias::new("created_at_epoch_seconds")).eq(cutoff_sec))
                        .add(Expr::col(Alias::new("created_at_epoch_nanos")).lte(cutoff_nanos)),
                ),
        )
        .to_owned();
    if let Some(values) = status.filter(|values| !values.is_empty()) {
        statement.and_where(
            Expr::col(Alias::new("status")).is_in(values.iter().map(ToString::to_string)),
        );
    }
    let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(pool.clone());
    db.execute_raw(db.get_database_backend().build(&statement))
        .await?;
    Ok(())
}

/// Requeues processing jobs whose update instant is at or before the cutoff.
///
/// # Errors
/// Returns an error if the requeue query fails.
pub async fn requeue_through(
    pool: &SqlitePool,
    cutoff_sec: i64,
    cutoff_nanos: i32,
    updated_at: String,
    now_sec: i64,
    now_nanos: i32,
) -> loco_rs::Result<()> {
    let statement = Query::update()
        .table(Alias::new("sqlt_loco_queue"))
        .values([
            (
                Alias::new("status"),
                Expr::val(JobStatus::Queued.to_string()),
            ),
            (Alias::new("updated_at"), Expr::val(updated_at)),
            (Alias::new("updated_at_epoch_seconds"), Expr::val(now_sec)),
            (Alias::new("updated_at_epoch_nanos"), Expr::val(now_nanos)),
            (Alias::new("updated_at_legacy_checked"), Expr::val(1)),
        ])
        .and_where(Expr::col(Alias::new("status")).eq(JobStatus::Processing.to_string()))
        .cond_where(
            Condition::any()
                .add(Expr::col(Alias::new("updated_at_epoch_seconds")).lt(cutoff_sec))
                .add(
                    Condition::all()
                        .add(Expr::col(Alias::new("updated_at_epoch_seconds")).eq(cutoff_sec))
                        .add(Expr::col(Alias::new("updated_at_epoch_nanos")).lte(cutoff_nanos)),
                ),
        )
        .to_owned();
    let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(pool.clone());
    db.execute_raw(db.get_database_backend().build(&statement))
        .await?;
    Ok(())
}

/// Lists jobs created at or before the supplied exact instant.
///
/// # Errors
/// Returns an error if the age query fails.
pub async fn get_jobs_at_cutoff(
    pool: &SqlitePool,
    status: Option<&Vec<JobStatus>>,
    cutoff_sec: i64,
    cutoff_nanos: i32,
) -> loco_rs::Result<Vec<Job>> {
    let mut statement = queue_listing();
    statement.cond_where(
        Condition::any()
            .add(Expr::col(Alias::new("created_at_epoch_seconds")).lt(cutoff_sec))
            .add(
                Condition::all()
                    .add(Expr::col(Alias::new("created_at_epoch_seconds")).eq(cutoff_sec))
                    .add(Expr::col(Alias::new("created_at_epoch_nanos")).lte(cutoff_nanos)),
            ),
    );
    list_queue_rows(pool, statement, status).await
}

async fn initialize_schema(
    pool: &SqlitePool,
    _timezone: crate::time::Timezone,
) -> loco_rs::Result<()> {
    loco_rs::bgworker::sqlt::initialize_database(pool).await?;
    Ok(())
}

async fn ensure_exact_columns(pool: &SqlitePool) -> loco_rs::Result<()> {
    use sea_orm::sea_query::{ColumnDef, Func, Table};
    let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(pool.clone());
    for name in [
        "run_at_epoch_seconds",
        "run_at_epoch_nanos",
        "run_at_legacy_checked",
        "created_at_epoch_seconds",
        "created_at_epoch_nanos",
        "created_at_legacy_checked",
        "updated_at_epoch_seconds",
        "updated_at_epoch_nanos",
        "updated_at_legacy_checked",
    ] {
        let exists = Query::select()
            .expr(Expr::val(1))
            .from_function(
                Func::cust(Alias::new("pragma_table_info")).arg("sqlt_loco_queue"),
                Alias::new("info"),
            )
            .and_where(Expr::col(Alias::new("name")).eq(name))
            .to_owned();
        if db
            .query_one_raw(db.get_database_backend().build(&exists))
            .await?
            .is_none()
        {
            let mut column = ColumnDef::new(Alias::new(name));
            column.integer();
            if name.ends_with("_checked") {
                column.not_null().default(0);
            }
            let alter = Table::alter()
                .table(Alias::new("sqlt_loco_queue"))
                .add_column(column)
                .to_owned();
            db.execute_raw(db.get_database_backend().build(&alter))
                .await?;
        }
    }
    backfill_lifecycle_epochs(pool).await?;
    Ok(())
}

async fn ensure_queue_indexes(pool: &SqlitePool) -> loco_rs::Result<()> {
    use sea_orm::sea_query::{ConditionalStatement, Index, IndexOrder};
    let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(pool.clone());
    let exact = Index::create()
        .name("idx_sqlt_queue_status_priority_epoch")
        .table(Alias::new("sqlt_loco_queue"))
        .if_not_exists()
        .col(Alias::new("status"))
        .col((Alias::new("priority"), IndexOrder::Desc))
        .col(Alias::new("run_at_epoch_seconds"))
        .col(Alias::new("run_at_epoch_nanos"))
        .col(Alias::new("id"))
        .and_where(Expr::col(Alias::new("run_at_epoch_seconds")).is_not_null())
        .and_where(Expr::col(Alias::new("run_at_epoch_nanos")).is_not_null())
        .to_owned();
    let legacy = Index::create()
        .name("idx_sqlt_queue_legacy_unchecked")
        .table(Alias::new("sqlt_loco_queue"))
        .if_not_exists()
        .col(Alias::new("id"))
        .and_where(Expr::col(Alias::new("run_at_epoch_seconds")).is_null())
        .and_where(Expr::col(Alias::new("run_at_legacy_checked")).eq(0))
        .to_owned();
    db.execute_raw(db.get_database_backend().build(&exact))
        .await?;
    db.execute_raw(db.get_database_backend().build(&legacy))
        .await?;
    Ok(())
}

async fn backfill_lifecycle_epochs(pool: &SqlitePool) -> loco_rs::Result<()> {
    for (column, sec_column, nanos_column, checked_column) in [
        (
            "created_at",
            "created_at_epoch_seconds",
            "created_at_epoch_nanos",
            "created_at_legacy_checked",
        ),
        (
            "updated_at",
            "updated_at_epoch_seconds",
            "updated_at_epoch_nanos",
            "updated_at_legacy_checked",
        ),
    ] {
        loop {
            let select = Query::select()
                .column(Alias::new("id"))
                .expr(
                    sea_orm::sea_query::Func::cust(Alias::new("quote"))
                        .arg(Expr::col(Alias::new(column))),
                )
                .from(Alias::new("sqlt_loco_queue"))
                .and_where(Expr::col(Alias::new(sec_column)).is_null())
                .and_where(Expr::col(Alias::new(checked_column)).eq(0))
                .order_by(Alias::new("id"), sea_orm::Order::Asc)
                .limit(1000)
                .to_owned();
            let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(pool.clone());
            let rows = db
                .query_all_raw(db.get_database_backend().build(&select))
                .await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let id: String = row.try_get_by_index(0)?;
                let quoted: String = row.try_get_by_index(1)?;
                let raw = quoted
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
                    .unwrap_or(&quoted);
                let mut update = Query::update()
                    .table(Alias::new("sqlt_loco_queue"))
                    .value(Alias::new(checked_column), 1)
                    .and_where(Expr::col(Alias::new("id")).eq(id.clone()))
                    .and_where(Expr::col(Alias::new(sec_column)).is_null())
                    .to_owned();
                if let Some(at) = parse_storage(raw) {
                    let (seconds, nanos) = exact_parts(at);
                    update.values([
                        (Alias::new(sec_column), Expr::val(seconds)),
                        (Alias::new(nanos_column), Expr::val(nanos)),
                    ]);
                } else {
                    tracing::error!(job_id=%id, column, value=%raw, "invalid legacy queue lifecycle timestamp; leaving row unkeyed");
                }
                db.execute_raw(db.get_database_backend().build(&update))
                    .await?;
            }
        }
    }
    Ok(())
}

/// Claims a due SQLite job matching the supplied tags.
///
/// # Errors
/// Returns an error if the database claim fails.
pub async fn dequeue(
    pool: &SqlitePool,
    tags: &[String],
    timezone: crate::time::Timezone,
) -> loco_rs::Result<Option<Job>> {
    use sea_orm::sea_query::{Func, Order};
    let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(pool.clone());
    let tx = crate::db::begin_write(&db).await?;
    let now = crate::time::queue_now(timezone).with_timezone(&Utc);
    let (sec, nanos) = exact_parts(now);
    let updated_at = crate::time::queue_storage(now, timezone);
    // Key legacy rows in bounded batches; no lexical wall-time shortcut is valid for mixed offsets.
    let legacy = Query::select()
        .column(Alias::new("id"))
        .expr(Func::cust(Alias::new("quote")).arg(Expr::col(Alias::new("run_at"))))
        .from(Alias::new("sqlt_loco_queue"))
        .and_where(Expr::col(Alias::new("run_at_epoch_seconds")).is_null())
        .and_where(Expr::col(Alias::new("run_at_legacy_checked")).eq(0))
        .order_by(Alias::new("id"), Order::Asc)
        .limit(1000)
        .to_owned();
    for row in tx
        .query_all_raw(tx.get_database_backend().build(&legacy))
        .await?
    {
        let id: String = row.try_get_by_index(0)?;
        let stored: String = row.try_get_by_index(1)?;
        let raw = stored
            .strip_prefix('\'')
            .and_then(|value| value.strip_suffix('\''))
            .unwrap_or(&stored);
        let mut update = Query::update()
            .table(Alias::new("sqlt_loco_queue"))
            .value(Alias::new("run_at_legacy_checked"), 1)
            .and_where(Expr::col(Alias::new("id")).eq(id.clone()))
            .and_where(Expr::col(Alias::new("run_at_epoch_seconds")).is_null())
            .to_owned();
        if let Some(at) = parse_storage(raw) {
            let (legacy_sec, legacy_nanos) = exact_parts(at);
            update.values([
                (Alias::new("run_at_epoch_seconds"), Expr::val(legacy_sec)),
                (Alias::new("run_at_epoch_nanos"), Expr::val(legacy_nanos)),
            ]);
        } else {
            tracing::error!(job_id=%id, run_at=%raw, "invalid legacy queue timestamp; leaving job queued");
        }
        tx.execute_raw(tx.get_database_backend().build(&update))
            .await?;
    }
    let mut select = queue_listing();
    select
        .and_where(Expr::col(Alias::new("status")).eq(JobStatus::Queued.to_string()))
        .and_where(Expr::col(Alias::new("run_at_epoch_seconds")).is_not_null())
        .and_where(Expr::col(Alias::new("run_at_epoch_nanos")).is_not_null())
        .and_where(Expr::col(Alias::new("run_at_epoch_seconds")).lte(sec))
        .cond_where(
            Condition::any()
                .add(Expr::col(Alias::new("run_at_epoch_seconds")).lt(sec))
                .add(Expr::col(Alias::new("run_at_epoch_nanos")).lte(nanos)),
        );
    if tags.is_empty() {
        select.and_where(Expr::col(Alias::new("tags")).is_null());
    } else {
        let matches = Query::select()
            .expr(Expr::val(1))
            .from_function(
                Func::cust(Alias::new("json_each")).arg(Expr::col(Alias::new("tags"))),
                Alias::new("tag"),
            )
            .and_where(
                Expr::col((Alias::new("tag"), Alias::new("value"))).is_in(tags.iter().cloned()),
            )
            .to_owned();
        select.and_where(Expr::exists(matches));
    }
    select
        .order_by(Alias::new("priority"), Order::Desc)
        .order_by(Alias::new("run_at_epoch_seconds"), Order::Asc)
        .order_by(Alias::new("run_at_epoch_nanos"), Order::Asc)
        .order_by(Alias::new("id"), Order::Asc)
        .limit(1);
    let row = tx
        .query_one_raw(tx.get_database_backend().build(&select))
        .await?;
    let job = row
        .map(|row| {
            row.try_as_sqlite_row()
                .ok_or_else(|| loco_rs::Error::Message("expected SQLite queue row".into()))
                .and_then(row_to_job)
        })
        .transpose()?;
    if let Some(job) = &job {
        let update = Query::update()
            .table(Alias::new("sqlt_loco_queue"))
            .values([
                (
                    Alias::new("status"),
                    Expr::val(JobStatus::Processing.to_string()),
                ),
                (Alias::new("updated_at"), Expr::val(updated_at)),
                (Alias::new("updated_at_epoch_seconds"), Expr::val(sec)),
                (Alias::new("updated_at_epoch_nanos"), Expr::val(nanos)),
                (Alias::new("updated_at_legacy_checked"), Expr::val(1)),
            ])
            .and_where(Expr::col(Alias::new("id")).eq(job.id.clone()))
            .to_owned();
        tx.execute_raw(tx.get_database_backend().build(&update))
            .await?;
    }
    tx.commit().await?;
    Ok(job)
}

fn row_to_job(row: &SqliteRow) -> loco_rs::Result<Job> {
    let data: JsonValue = row.try_get("task_data")?;
    let status: String = row.try_get("status")?;
    let stored: String = row.try_get("run_at")?;
    let at = parse_storage(&stored)
        .ok_or_else(|| loco_rs::Error::Message("invalid queue run_at".into()))?;
    let tags: Option<JsonValue> = row.try_get("tags").ok();
    let tags = tags
        .and_then(|value| serde_json::from_value::<Vec<String>>(value).ok())
        .filter(|tags| !tags.is_empty());
    Ok(Job {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        data,
        status: status.parse().map_err(loco_rs::Error::Message)?,
        run_at: at,
        interval: row.try_get("interval")?,
        created_at: row.try_get("created_at").ok(),
        updated_at: row.try_get("updated_at").ok(),
        tags,
        priority: row.try_get("priority")?,
    })
}

/// Marks a job complete and schedules its next occurrence when recurring.
///
/// # Errors
/// Returns an error if the completion update fails.
async fn complete(
    pool: &SqlitePool,
    id: &str,
    interval: Option<i64>,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<()> {
    let now = crate::time::queue_now(timezone).with_timezone(&Utc);
    let updated_at = crate::time::queue_storage(now, timezone);
    let (updated_sec, updated_nanos) = exact_parts(now);
    let mut statement = Query::update()
        .table(Alias::new("sqlt_loco_queue"))
        .values([
            (
                Alias::new("status"),
                Expr::val(
                    if interval.is_some() {
                        JobStatus::Queued
                    } else {
                        JobStatus::Completed
                    }
                    .to_string(),
                ),
            ),
            (Alias::new("updated_at"), Expr::val(updated_at)),
            (
                Alias::new("updated_at_epoch_seconds"),
                Expr::val(updated_sec),
            ),
            (
                Alias::new("updated_at_epoch_nanos"),
                Expr::val(updated_nanos),
            ),
            (Alias::new("updated_at_legacy_checked"), Expr::val(1)),
        ])
        .and_where(Expr::col(Alias::new("id")).eq(id))
        .to_owned();
    if let Some(interval) = interval {
        let next = now + chrono::Duration::milliseconds(interval);
        let (sec, nanos) = exact_parts(next);
        statement.values([
            (
                Alias::new("run_at"),
                Expr::val(crate::time::queue_storage(next, timezone)),
            ),
            (Alias::new("run_at_epoch_seconds"), Expr::val(sec)),
            (Alias::new("run_at_epoch_nanos"), Expr::val(nanos)),
        ]);
    }
    let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(pool.clone());
    db.execute_raw(db.get_database_backend().build(&statement))
        .await?;
    Ok(())
}

/// Stores a job failure and its error details.
///
/// # Errors
/// Returns an error if the failure update fails.
async fn fail(
    pool: &SqlitePool,
    id: &str,
    error: &loco_rs::Error,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<()> {
    let now = crate::time::queue_now(timezone).with_timezone(&Utc);
    let updated_at = crate::time::queue_storage(now, timezone);
    let (updated_sec, updated_nanos) = exact_parts(now);
    let statement = Query::update()
        .table(Alias::new("sqlt_loco_queue"))
        .values([
            (
                Alias::new("status"),
                Expr::val(JobStatus::Failed.to_string()),
            ),
            (Alias::new("updated_at"), Expr::val(updated_at)),
            (
                Alias::new("updated_at_epoch_seconds"),
                Expr::val(updated_sec),
            ),
            (
                Alias::new("updated_at_epoch_nanos"),
                Expr::val(updated_nanos),
            ),
            (Alias::new("updated_at_legacy_checked"), Expr::val(1)),
            (
                Alias::new("task_data"),
                sea_orm::sea_query::Func::cust(Alias::new("json_patch"))
                    .args([
                        Expr::col(Alias::new("task_data")),
                        Expr::val(serde_json::json!({"error": error.to_string()})),
                    ])
                    .into(),
            ),
        ])
        .and_where(Expr::col(Alias::new("id")).eq(id))
        .to_owned();
    let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(pool.clone());
    db.execute_raw(db.get_database_backend().build(&statement))
        .await?;
    Ok(())
}

async fn get_jobs(
    pool: &SqlitePool,
    status: Option<&Vec<JobStatus>>,
    age_days: Option<i64>,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<Vec<Job>> {
    if let Some(age_days) = age_days {
        backfill_lifecycle_epochs(pool).await?;
        let cutoff =
            crate::time::queue_now(timezone).with_timezone(&Utc) - chrono::Duration::days(age_days);
        let (sec, nanos) = exact_parts(cutoff);
        return get_jobs_at_cutoff(pool, status, sec, nanos).await;
    }
    list_queue_rows(pool, queue_listing(), status).await
}

fn queue_listing() -> sea_orm::sea_query::SelectStatement {
    Query::select()
        .columns(
            [
                "id",
                "name",
                "task_data",
                "status",
                "run_at",
                "interval",
                "created_at",
                "updated_at",
                "tags",
                "priority",
            ]
            .map(Alias::new),
        )
        .from(Alias::new("sqlt_loco_queue"))
        .to_owned()
}

async fn list_queue_rows(
    pool: &SqlitePool,
    mut statement: sea_orm::sea_query::SelectStatement,
    status: Option<&Vec<JobStatus>>,
) -> loco_rs::Result<Vec<Job>> {
    if let Some(status) = status.filter(|values| !values.is_empty()) {
        statement.and_where(
            Expr::col(Alias::new("status")).is_in(status.iter().map(ToString::to_string)),
        );
    }
    let db = sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(pool.clone());
    let rows = db
        .query_all_raw(db.get_database_backend().build(&statement))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let row = row.try_as_sqlite_row()?;
            match row_to_job(row) {
                Ok(job) => Some(job),
                Err(error) => {
                    tracing::error!(%error, "invalid queue row skipped by get_jobs");
                    None
                }
            }
        })
        .collect())
}
