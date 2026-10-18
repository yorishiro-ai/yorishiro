use std::{collections::HashMap, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use loco_rs::prelude::async_trait;
use loco_rs::{
    bgworker::{Job, JobHandler, JobId, JobStatus, QueueProvider},
    config::SqliteQueueConfig,
};
use sea_orm::sqlx::{
    self, AssertSqlSafe, ConnectOptions, QueryBuilder, Row,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow},
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
        sqlx::query("INSERT INTO sqlt_loco_queue (id, task_data, name, run_at, run_at_epoch_seconds, run_at_epoch_nanos, interval, tags, priority, created_at, created_at_epoch_seconds, created_at_epoch_nanos, created_at_legacy_checked, updated_at, updated_at_epoch_seconds, updated_at_epoch_nanos, updated_at_legacy_checked) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, 1)")
            .bind(&id).bind(args).bind(class).bind(&now_local).bind(sec).bind(nanos).bind(None::<i64>).bind(tags).bind(priority.unwrap_or(0)).bind(&now_local).bind(sec).bind(nanos).bind(&now_local).bind(sec).bind(nanos).execute(&self.pool).await?;
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
        let mut tx = self.pool.begin().await?;
        let mut id_iter = ids.iter();
        for chunk in jobs.chunks(1000) {
            let mut query = QueryBuilder::<sqlx::Sqlite>::new(
                "INSERT INTO sqlt_loco_queue (id, task_data, name, run_at, run_at_epoch_seconds, run_at_epoch_nanos, interval, tags, priority, created_at, created_at_epoch_seconds, created_at_epoch_nanos, created_at_legacy_checked, updated_at, updated_at_epoch_seconds, updated_at_epoch_nanos, updated_at_legacy_checked) ",
            );
            query.push_values(chunk, |mut row, (data, priority)| {
                let id = id_iter.next().expect("one id per batch entry").clone();
                row.push_bind(id)
                    .push_bind(data)
                    .push_bind(&class)
                    .push_bind(&run_at)
                    .push_bind(sec)
                    .push_bind(nanos)
                    .push_bind(None::<i64>)
                    .push_bind(tags.clone())
                    .push_bind(priority.unwrap_or(0))
                    .push_bind(&now_local)
                    .push_bind(sec)
                    .push_bind(nanos)
                    .push_bind(1)
                    .push_bind(&now_local)
                    .push_bind(sec)
                    .push_bind(nanos)
                    .push_bind(1);
            });
            query.build().execute(&mut *tx).await?;
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
        let updated_at = crate::time::queue_storage(now, self.timezone);
        let (sec, nanos) = exact_parts(now);
        sqlx::query("UPDATE sqlt_loco_queue SET status=?, updated_at=?, updated_at_epoch_seconds=?, updated_at_epoch_nanos=?, updated_at_legacy_checked=1 WHERE name=? AND status=?")
            .bind(JobStatus::Cancelled.to_string())
            .bind(updated_at)
            .bind(sec)
            .bind(nanos)
            .bind(name)
            .bind(JobStatus::Queued.to_string())
            .execute(&self.pool)
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
        let result = if let Some(id) = id {
            sqlx::query("UPDATE sqlt_loco_queue SET status=?, updated_at=?, updated_at_epoch_seconds=?, updated_at_epoch_nanos=?, updated_at_legacy_checked=1, run_at=?, run_at_epoch_seconds=?, run_at_epoch_nanos=? WHERE status=? AND id=?")
                .bind(JobStatus::Queued.to_string()).bind(&updated_at).bind(sec).bind(nanos).bind(local).bind(sec).bind(nanos).bind(JobStatus::Failed.to_string()).bind(id).execute(&self.pool).await?
        } else {
            sqlx::query("UPDATE sqlt_loco_queue SET status=?, updated_at=?, updated_at_epoch_seconds=?, updated_at_epoch_nanos=?, updated_at_legacy_checked=1, run_at=?, run_at_epoch_seconds=?, run_at_epoch_nanos=? WHERE status=?")
                .bind(JobStatus::Queued.to_string()).bind(&updated_at).bind(sec).bind(nanos).bind(local).bind(sec).bind(nanos).bind(JobStatus::Failed.to_string()).execute(&self.pool).await?
        };
        Ok(result.rows_affected())
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
    let mut query = String::from(
        "DELETE FROM sqlt_loco_queue WHERE created_at_epoch_seconds IS NOT NULL AND (created_at_epoch_seconds < ? OR (created_at_epoch_seconds = ? AND created_at_epoch_nanos <= ?))",
    );
    if status.is_some_and(|values| !values.is_empty()) {
        query.push_str(" AND status IN (");
        query.push_str(&vec!["?"; status.map_or(0, Vec::len)].join(","));
        query.push(')');
    }
    let mut statement = sqlx::query(AssertSqlSafe(query))
        .bind(cutoff_sec)
        .bind(cutoff_sec)
        .bind(cutoff_nanos);
    if let Some(values) = status.filter(|values| !values.is_empty()) {
        for value in values {
            statement = statement.bind(value.to_string());
        }
    }
    statement.execute(pool).await?;
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
    sqlx::query("UPDATE sqlt_loco_queue SET status=?, updated_at=?, updated_at_epoch_seconds=?, updated_at_epoch_nanos=?, updated_at_legacy_checked=1 WHERE status=? AND updated_at_epoch_seconds IS NOT NULL AND (updated_at_epoch_seconds < ? OR (updated_at_epoch_seconds = ? AND updated_at_epoch_nanos <= ?))")
        .bind(JobStatus::Queued.to_string()).bind(updated_at).bind(now_sec).bind(now_nanos).bind(JobStatus::Processing.to_string()).bind(cutoff_sec).bind(cutoff_sec).bind(cutoff_nanos).execute(pool).await?;
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
    let mut query = String::from(
        "SELECT id, name, task_data, status, run_at, interval, created_at, updated_at, tags, priority FROM sqlt_loco_queue WHERE created_at_epoch_seconds IS NOT NULL AND (created_at_epoch_seconds < ? OR (created_at_epoch_seconds = ? AND created_at_epoch_nanos <= ?))",
    );
    if status.is_some_and(|values| !values.is_empty()) {
        query.push_str(" AND status IN (");
        query.push_str(&vec!["?"; status.map_or(0, Vec::len)].join(","));
        query.push(')');
    }
    let mut statement = sqlx::query(AssertSqlSafe(query))
        .bind(cutoff_sec)
        .bind(cutoff_sec)
        .bind(cutoff_nanos);
    if let Some(values) = status.filter(|values| !values.is_empty()) {
        for value in values {
            statement = statement.bind(value.to_string());
        }
    }
    let rows = statement.fetch_all(pool).await?;
    Ok(rows
        .iter()
        .filter_map(|row| match row_to_job(row) {
            Ok(job) => Some(job),
            Err(error) => {
                tracing::error!(%error, "invalid queue row skipped by get_jobs");
                None
            }
        })
        .collect())
}

async fn initialize_schema(
    pool: &SqlitePool,
    _timezone: crate::time::Timezone,
) -> loco_rs::Result<()> {
    loco_rs::bgworker::sqlt::initialize_database(pool).await?;
    Ok(())
}

async fn ensure_exact_columns(pool: &SqlitePool) -> loco_rs::Result<()> {
    for (name, definition) in [
        ("run_at_epoch_seconds", "INTEGER"),
        ("run_at_epoch_nanos", "INTEGER"),
        ("run_at_legacy_checked", "INTEGER NOT NULL DEFAULT 0"),
        ("created_at_epoch_seconds", "INTEGER"),
        ("created_at_epoch_nanos", "INTEGER"),
        ("created_at_legacy_checked", "INTEGER NOT NULL DEFAULT 0"),
        ("updated_at_epoch_seconds", "INTEGER"),
        ("updated_at_epoch_nanos", "INTEGER"),
        ("updated_at_legacy_checked", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pragma_table_info('sqlt_loco_queue') WHERE name = ?)",
        )
        .bind(name)
        .fetch_one(pool)
        .await?;
        if !exists {
            sqlx::query(AssertSqlSafe(format!(
                "ALTER TABLE sqlt_loco_queue ADD COLUMN {name} {definition}"
            )))
            .execute(pool)
            .await?;
        }
    }
    backfill_lifecycle_epochs(pool).await?;
    Ok(())
}

async fn ensure_queue_indexes(pool: &SqlitePool) -> loco_rs::Result<()> {
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_sqlt_queue_status_priority_epoch ON sqlt_loco_queue(status, priority DESC, run_at_epoch_seconds, run_at_epoch_nanos, id) WHERE run_at_epoch_seconds IS NOT NULL AND run_at_epoch_nanos IS NOT NULL").execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_sqlt_queue_legacy_unchecked ON sqlt_loco_queue(id) WHERE run_at_epoch_seconds IS NULL AND run_at_legacy_checked=0").execute(pool).await?;
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
            let select = format!(
                "SELECT id, quote({column}) FROM sqlt_loco_queue WHERE {sec_column} IS NULL AND {checked_column}=0 ORDER BY id LIMIT 1000"
            );
            let rows: Vec<(String, String)> = sqlx::query_as(AssertSqlSafe(select))
                .fetch_all(pool)
                .await?;
            if rows.is_empty() {
                break;
            }
            for (id, quoted) in rows {
                let raw = quoted
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
                    .unwrap_or(&quoted);
                if let Some(at) = parse_storage(raw) {
                    let (seconds, nanos) = exact_parts(at);
                    let update = format!(
                        "UPDATE sqlt_loco_queue SET {sec_column}=?, {nanos_column}=?, {checked_column}=1 WHERE id=? AND {sec_column} IS NULL"
                    );
                    sqlx::query(AssertSqlSafe(update))
                        .bind(seconds)
                        .bind(nanos)
                        .bind(&id)
                        .execute(pool)
                        .await?;
                } else {
                    let update = format!(
                        "UPDATE sqlt_loco_queue SET {checked_column}=1 WHERE id=? AND {sec_column} IS NULL"
                    );
                    sqlx::query(AssertSqlSafe(update))
                        .bind(&id)
                        .execute(pool)
                        .await?;
                    tracing::error!(job_id=%id, column, value=%raw, "invalid legacy queue lifecycle timestamp; leaving row unkeyed");
                }
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
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    // Key legacy rows in bounded batches; no lexical wall-time shortcut is valid for mixed offsets.
    let now = crate::time::queue_now(timezone).with_timezone(&Utc);
    let (sec, nanos) = exact_parts(now);
    let updated_at = crate::time::queue_storage(now, timezone);
    let legacy: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, quote(run_at) FROM sqlt_loco_queue WHERE run_at_epoch_seconds IS NULL AND run_at_legacy_checked=0 ORDER BY id LIMIT 1000",
    )
    .fetch_all(&mut *tx)
    .await?;
    for (id, stored) in legacy {
        let raw = stored
            .strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))
            .unwrap_or(&stored);
        if let Some(at) = parse_storage(raw) {
            let (legacy_sec, legacy_nanos) = exact_parts(at);
            sqlx::query("UPDATE sqlt_loco_queue SET run_at_epoch_seconds=?, run_at_epoch_nanos=?, run_at_legacy_checked=1 WHERE id=? AND run_at_epoch_seconds IS NULL")
                .bind(legacy_sec).bind(legacy_nanos).bind(id).execute(&mut *tx).await?;
        } else {
            sqlx::query("UPDATE sqlt_loco_queue SET run_at_legacy_checked=1 WHERE id=? AND run_at_epoch_seconds IS NULL")
                .bind(&id).execute(&mut *tx).await?;
            tracing::error!(job_id=%id, run_at=%raw, "invalid legacy queue timestamp; leaving job queued");
        }
    }
    let mut query = String::from(
        "SELECT id, name, task_data, status, run_at, interval, tags, priority, run_at_epoch_seconds, run_at_epoch_nanos FROM sqlt_loco_queue WHERE status = ? AND run_at_epoch_seconds IS NOT NULL AND run_at_epoch_nanos IS NOT NULL AND run_at_epoch_seconds <= ? AND (run_at_epoch_seconds < ? OR run_at_epoch_nanos <= ?)",
    );
    if tags.is_empty() {
        query.push_str(" AND tags IS NULL");
    } else {
        query.push_str(" AND tags IS NOT NULL AND (");
        query.push_str(&vec!["json_extract(tags, '$') LIKE ?"; tags.len()].join(" OR "));
        query.push(')');
    }
    query.push_str(" ORDER BY priority DESC, run_at_epoch_seconds, run_at_epoch_nanos, id LIMIT 1");
    let mut q = sqlx::query(AssertSqlSafe(query))
        .bind(JobStatus::Queued.to_string())
        .bind(sec)
        .bind(sec)
        .bind(nanos);
    for tag in tags {
        q = q.bind(format!("%\"{tag}\"%"));
    }
    let row = q.fetch_optional(&mut *tx).await?;
    if let Some(row) = row {
        let job = row_to_job(&row)?;
        let (updated_sec, updated_nanos) = (sec, nanos);
        sqlx::query("UPDATE sqlt_loco_queue SET status=?, updated_at=?, updated_at_epoch_seconds=?, updated_at_epoch_nanos=?, updated_at_legacy_checked=1 WHERE id=?")
            .bind(JobStatus::Processing.to_string())
            .bind(updated_at)
            .bind(updated_sec)
            .bind(updated_nanos)
            .bind(&job.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(job))
    } else {
        tx.commit().await?;
        Ok(None)
    }
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
    if let Some(interval) = interval {
        let next = now + chrono::Duration::milliseconds(interval);
        let (sec, nanos) = exact_parts(next);
        let local = crate::time::queue_storage(next, timezone);
        sqlx::query("UPDATE sqlt_loco_queue SET status=?, updated_at=?, updated_at_epoch_seconds=?, updated_at_epoch_nanos=?, updated_at_legacy_checked=1, run_at=?, run_at_epoch_seconds=?, run_at_epoch_nanos=? WHERE id=?")
            .bind(JobStatus::Queued.to_string()).bind(updated_at).bind(updated_sec).bind(updated_nanos).bind(local).bind(sec).bind(nanos).bind(id).execute(pool).await?;
    } else {
        sqlx::query("UPDATE sqlt_loco_queue SET status=?, updated_at=?, updated_at_epoch_seconds=?, updated_at_epoch_nanos=?, updated_at_legacy_checked=1 WHERE id=?")
            .bind(JobStatus::Completed.to_string())
            .bind(updated_at)
            .bind(updated_sec)
            .bind(updated_nanos)
            .bind(id)
            .execute(pool)
            .await?;
    }
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
    sqlx::query("UPDATE sqlt_loco_queue SET status=?, updated_at=?, updated_at_epoch_seconds=?, updated_at_epoch_nanos=?, updated_at_legacy_checked=1, task_data=json_patch(task_data, ?) WHERE id=?")
        .bind(JobStatus::Failed.to_string()).bind(updated_at).bind(updated_sec).bind(updated_nanos).bind(serde_json::json!({"error": error.to_string()})).bind(id).execute(pool).await?;
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
    let mut query = String::from(
        "SELECT id, name, task_data, status, run_at, interval, created_at, updated_at, tags, priority FROM sqlt_loco_queue WHERE 1=1",
    );
    if let Some(status) = status.filter(|status| !status.is_empty()) {
        query.push_str(" AND status IN (");
        query.push_str(&vec!["?"; status.len()].join(","));
        query.push(')');
    }
    let mut statement = sqlx::query(AssertSqlSafe(query));
    if let Some(status) = status.filter(|status| !status.is_empty()) {
        for value in status {
            statement = statement.bind(value.to_string());
        }
    }
    let rows = statement.fetch_all(pool).await?;
    Ok(rows
        .iter()
        .filter_map(|row| match row_to_job(row) {
            Ok(job) => Some(job),
            Err(error) => {
                tracing::error!(%error, "invalid queue row skipped by get_jobs");
                None
            }
        })
        .collect())
}
