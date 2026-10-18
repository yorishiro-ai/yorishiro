use std::{collections::HashMap, sync::Arc, time::Duration};

use chrono::Utc;
use loco_rs::prelude::async_trait;
use loco_rs::{
    bgworker::{Job, JobHandler, JobId, JobStatus, QueueProvider},
    config::PostgresQueueConfig,
};
use sea_orm::sqlx::{
    self, AssertSqlSafe, ConnectOptions, Executor, PgPool, Postgres, QueryBuilder, Row,
    postgres::PgConnectOptions,
};
use serde_json::Value as JsonValue;
use tokio::sync::watch;

#[derive(Clone)]
pub struct PostgresTimezoneQueue {
    pub pool: PgPool,
    handlers: Arc<tokio::sync::Mutex<HashMap<String, Arc<JobHandler>>>>,
    num_workers: u32,
    poll_seconds: u32,
    reaper: Option<loco_rs::config::ReaperConfig>,
    timezone: crate::time::Timezone,
    token: watch::Sender<bool>,
}

impl PostgresTimezoneQueue {
    /// Creates a provider using the configured PostgreSQL queue settings.
    ///
    /// # Errors
    /// Returns an error when the pool cannot be configured or connected.
    pub async fn connect(
        config: &PostgresQueueConfig,
        timezone: crate::time::Timezone,
    ) -> loco_rs::Result<Self> {
        let mut options: PgConnectOptions = config.uri.parse()?;
        if !config.enable_logging {
            options = options.disable_statement_logging();
        }
        let pool = sqlx::postgres::PgPoolOptions::new()
            .min_connections(config.min_connections)
            .max_connections(config.max_connections)
            .idle_timeout(Duration::from_millis(config.idle_timeout))
            .acquire_timeout(Duration::from_millis(config.connect_timeout))
            .after_connect(move |connection, _| {
                Box::pin(async move {
                    let zone = timezone.name().replace('\'', "''");
                    connection
                        .execute(sqlx::AssertSqlSafe(format!("SET TIME ZONE '{zone}'")))
                        .await
                        .map(|_| ())
                })
            })
            .connect_with(options)
            .await?;
        Ok(Self {
            pool,
            handlers: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            num_workers: config.num_workers,
            poll_seconds: config.poll_interval_sec,
            reaper: config.reaper.clone(),
            token: watch::channel(false).0,
            timezone,
        })
    }

    /// Inserts a queue row with a caller-selected ID.
    ///
    /// # Errors
    /// Returns an error if the queue insert fails or the ID conflicts.
    pub async fn enqueue_with_id(
        &self,
        class: String,
        args: JsonValue,
        tags: Option<Vec<String>>,
        priority: Option<i32>,
        id: String,
    ) -> loco_rs::Result<String> {
        let now = crate::time::database_instant(
            crate::time::queue_now(self.timezone).with_timezone(&Utc),
        );
        let inserted = sqlx::query("INSERT INTO pg_loco_queue (id,task_data,name,run_at,interval,tags,priority,created_at,updated_at) VALUES ($1,$2,$3,$4,NULL,$5,$6,$4,$4) ON CONFLICT (id) DO NOTHING")
            .bind(&id).bind(args).bind(class).bind(now).bind(tags.map(serde_json::to_value).transpose()?).bind(priority.unwrap_or(0)).execute(&self.pool).await?;
        if inserted.rows_affected() == 0 {
            return Err(loco_rs::Error::from(
                crate::error::YorishiroError::Conflict {
                    message: format!("queue job ID already exists: {id}"),
                },
            ));
        }
        Ok(id)
    }
}

#[async_trait]
impl QueueProvider for PostgresTimezoneQueue {
    async fn enqueue(
        &self,
        class: String,
        _queue: Option<String>,
        args: JsonValue,
        tags: Option<Vec<String>>,
        priority: Option<i32>,
    ) -> loco_rs::Result<Option<String>> {
        let id = ulid::Ulid::new().to_string();
        Ok(Some(
            self.enqueue_with_id(class, args, tags, priority, id)
                .await?,
        ))
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
        let now = crate::time::database_instant(
            crate::time::queue_now(self.timezone).with_timezone(&Utc),
        );
        let tags = tags.map(serde_json::to_value).transpose()?;
        let mut ids = Vec::with_capacity(jobs.len());
        let mut unique_ids = std::collections::HashSet::with_capacity(jobs.len());
        for _ in &jobs {
            let id = ulid::Ulid::new().to_string();
            if !unique_ids.insert(id.clone()) {
                return Err(loco_rs::Error::Message(
                    "duplicate ULID generated within PostgreSQL batch".into(),
                ));
            }
            ids.push(id);
        }
        let mut tx = self.pool.begin().await?;
        let mut id_iter = ids.iter();
        for chunk in jobs.chunks(5000) {
            let mut query = QueryBuilder::<Postgres>::new(
                "INSERT INTO pg_loco_queue (id,task_data,name,run_at,interval,tags,priority,created_at,updated_at) ",
            );
            query.push_values(chunk, |mut row, (data, priority)| {
                let id = id_iter.next().expect("one id per batch entry").clone();
                row.push_bind(id)
                    .push_bind(data)
                    .push_bind(&class)
                    .push_bind(now)
                    .push_bind(None::<i64>)
                    .push_bind(tags.clone())
                    .push_bind(priority.unwrap_or(0))
                    .push_bind(now)
                    .push_bind(now);
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
        let reaper_handle = if let Some(reaper) = self.reaper.clone() {
            let pool = self.pool.clone();
            let mut token = self.token.subscribe();
            let timezone = self.timezone;
            Some(tokio::spawn(async move {
                loop {
                    tokio::select! {
                        changed = token.changed() => if changed.is_err() || *token.borrow() { break; },
                        () = tokio::time::sleep(Duration::from_secs(reaper.interval_seconds)) => {
                            if let Err(err) = requeue(&pool, reaper.age_minutes, timezone).await { tracing::error!(%err, "queue reaper failed"); }
                        }
                    }
                }
            }))
        } else {
            None
        };
        let mut workers = Vec::new();
        for worker_index in 0..self.num_workers {
            let pool = self.pool.clone();
            let handlers = self.handlers.clone();
            let tags = tags.clone();
            let mut token = self.token.subscribe();
            let poll = self.poll_seconds;
            let timezone = self.timezone;
            workers.push(tokio::spawn(async move {
                loop {
                    if *token.borrow() { break; }
                    match dequeue(&pool, &tags, timezone).await {
                        Ok(Some(job)) => if let Some(handler) = handlers.lock().await.get(&job.name).cloned() {
                            match handler(job.id.clone(), job.data.clone()).await {
                                Ok(()) => if let Err(err) = complete(&pool, &job.id, job.interval, timezone).await { tracing::error!(%err, job_id=%job.id, "queue completion failed"); },
                                Err(err) => if let Err(fail_err) = fail(&pool, &job.id, &err, timezone).await { tracing::error!(%fail_err, job_id=%job.id, "queue failure failed"); },
                            }
                        } else {
                            tracing::error!(worker_index, job_name=%job.name, "No handler registered for job");
                        },
                        Ok(None) => tokio::select! { biased; changed = token.changed() => if changed.is_err() || *token.borrow() { break; }, () = tokio::time::sleep(Duration::from_secs(poll.into())) => {} },
                        Err(err) => { tracing::error!(%err, "queue dequeue failed"); tokio::select! { () = tokio::time::sleep(Duration::from_secs(1)) => {}, changed = token.changed() => if changed.is_err() || *token.borrow() { break; } } },
                    }
                }
            }));
        }
        for worker in workers {
            match worker.await {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {
                    tracing::debug!("PostgreSQL queue worker cancelled")
                }
                Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                Err(error) => return Err(loco_rs::Error::Message(error.to_string())),
            }
        }
        if let Some(handle) = reaper_handle {
            let _ = handle.await;
        }
        Ok(())
    }
    async fn setup(&self) -> loco_rs::Result<()> {
        loco_rs::bgworker::pg::initialize_database(&self.pool).await?;
        sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS pg_loco_queue_id_key ON pg_loco_queue (id)")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    async fn clear(&self) -> loco_rs::Result<()> {
        loco_rs::bgworker::pg::clear(&self.pool).await?;
        Ok(())
    }
    async fn ping(&self) -> loco_rs::Result<()> {
        loco_rs::bgworker::pg::ping(&self.pool).await?;
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
        let now = crate::time::database_instant(
            crate::time::queue_now(self.timezone).with_timezone(&Utc),
        );
        sqlx::query("UPDATE pg_loco_queue SET status=$1,updated_at=$2 WHERE name=$3 AND status=$4")
            .bind(JobStatus::Cancelled.to_string())
            .bind(now)
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
        let mut q = String::from("DELETE FROM pg_loco_queue WHERE status IN (");
        q.push_str(
            &(1..=status.len())
                .map(|i| format!("${i}"))
                .collect::<Vec<_>>()
                .join(","),
        );
        q.push(')');
        let mut query = sqlx::query(AssertSqlSafe(q));
        for value in status {
            query = query.bind(value.to_string());
        }
        query.execute(&self.pool).await?;
        Ok(())
    }
    async fn clear_jobs_older_than(
        &self,
        age_days: i64,
        status: Option<&Vec<JobStatus>>,
    ) -> loco_rs::Result<()> {
        let cutoff = crate::time::queue_now(self.timezone).with_timezone(&Utc)
            - chrono::Duration::days(age_days);
        let mut q = String::from("DELETE FROM pg_loco_queue WHERE created_at <= $1");
        if status.is_some_and(|s| !s.is_empty()) {
            q.push_str(" AND status IN (");
            q.push_str(
                &(2..=status.unwrap().len() + 1)
                    .map(|i| format!("${i}"))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            q.push(')');
        }
        let mut query = sqlx::query(AssertSqlSafe(q)).bind(cutoff);
        if let Some(values) = status.filter(|s| !s.is_empty()) {
            for v in values {
                query = query.bind(v.to_string());
            }
        }
        query.execute(&self.pool).await?;
        Ok(())
    }
    async fn retry_failed(&self, id: Option<&str>) -> loco_rs::Result<u64> {
        let now = crate::time::database_instant(
            crate::time::queue_now(self.timezone).with_timezone(&Utc),
        );
        let result = if let Some(id) = id {
            sqlx::query("UPDATE pg_loco_queue SET status=$1,updated_at=$2,run_at=$3 WHERE status=$4 AND id=$5").bind(JobStatus::Queued.to_string()).bind(now).bind(now).bind(JobStatus::Failed.to_string()).bind(id).execute(&self.pool).await?
        } else {
            sqlx::query(
                "UPDATE pg_loco_queue SET status=$1,updated_at=$2,run_at=$3 WHERE status=$4",
            )
            .bind(JobStatus::Queued.to_string())
            .bind(now)
            .bind(now)
            .bind(JobStatus::Failed.to_string())
            .execute(&self.pool)
            .await?
        };
        Ok(result.rows_affected())
    }
    async fn requeue(&self, age_minutes: &i64) -> loco_rs::Result<()> {
        requeue(&self.pool, *age_minutes, self.timezone).await
    }
    fn describe(&self) -> String {
        "postgres queue".into()
    }
    fn shutdown(&self) -> loco_rs::Result<()> {
        let _ = self.token.send(true);
        Ok(())
    }
}

/// Claims a due PostgreSQL job matching the supplied tags.
///
/// # Errors
/// Returns an error if the database claim fails.
async fn dequeue(
    pool: &PgPool,
    tags: &[String],
    timezone: crate::time::Timezone,
) -> loco_rs::Result<Option<Job>> {
    let mut tx = pool.begin().await?;
    let now = crate::time::queue_now(timezone).with_timezone(&Utc);
    let mut query = String::from(
        "SELECT id,name,task_data,status,run_at,interval,created_at,updated_at,tags,priority FROM pg_loco_queue WHERE status=$1 AND run_at <= $2",
    );
    if tags.is_empty() {
        query.push_str(" AND tags IS NULL");
    } else {
        query.push_str(" AND tags IS NOT NULL AND (");
        query.push_str(
            &(3..=tags.len() + 2)
                .map(|i| format!("(tags)::jsonb ? ${i}"))
                .collect::<Vec<_>>()
                .join(" OR "),
        );
        query.push(')');
    }
    query.push_str(" ORDER BY priority DESC,run_at,id LIMIT 1 FOR UPDATE SKIP LOCKED");
    let mut q = sqlx::query(AssertSqlSafe(query))
        .bind(JobStatus::Queued.to_string())
        .bind(now);
    for tag in tags {
        q = q.bind(tag);
    }
    if let Some(row) = q.fetch_optional(&mut *tx).await? {
        let job = row_to_job(&row)?;
        sqlx::query("UPDATE pg_loco_queue SET status=$1,updated_at=$2 WHERE id=$3")
            .bind(JobStatus::Processing.to_string())
            .bind(now)
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

fn row_to_job(row: &sea_orm::sqlx::postgres::PgRow) -> loco_rs::Result<Job> {
    let status: String = row.try_get("status")?;
    let tags: Option<JsonValue> = row.try_get("tags").ok();
    Ok(Job {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        data: row.try_get("task_data")?,
        status: status.parse().map_err(loco_rs::Error::Message)?,
        run_at: row.try_get("run_at")?,
        interval: row.try_get("interval")?,
        created_at: row.try_get("created_at").ok(),
        updated_at: row.try_get("updated_at").ok(),
        tags: tags.and_then(|v| serde_json::from_value(v).ok()),
        priority: row.try_get("priority")?,
    })
}

/// Marks a job complete and schedules its next occurrence when recurring.
///
/// # Errors
/// Returns an error if the completion update fails.
async fn complete(
    pool: &PgPool,
    id: &str,
    interval: Option<i64>,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<()> {
    let now = crate::time::database_instant(crate::time::queue_now(timezone).with_timezone(&Utc));
    if let Some(ms) = interval {
        let next = now + chrono::Duration::milliseconds(ms);
        sqlx::query("UPDATE pg_loco_queue SET status=$1,updated_at=$2,run_at=$3 WHERE id=$4")
            .bind(JobStatus::Queued.to_string())
            .bind(now)
            .bind(next.with_timezone(&Utc))
            .bind(id)
            .execute(pool)
            .await?;
    } else {
        sqlx::query("UPDATE pg_loco_queue SET status=$1,updated_at=$2 WHERE id=$3")
            .bind(JobStatus::Completed.to_string())
            .bind(now)
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
    pool: &PgPool,
    id: &str,
    error: &loco_rs::Error,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<()> {
    sqlx::query("UPDATE pg_loco_queue SET status=$1,updated_at=$2,task_data=task_data || $3::jsonb WHERE id=$4").bind(JobStatus::Failed.to_string()).bind(crate::time::database_instant(crate::time::queue_now(timezone).with_timezone(&Utc))).bind(serde_json::json!({"error":error.to_string()})).bind(id).execute(pool).await?;
    Ok(())
}
async fn requeue(pool: &PgPool, age: i64, timezone: crate::time::Timezone) -> loco_rs::Result<()> {
    let now = crate::time::database_instant(crate::time::queue_now(timezone).with_timezone(&Utc));
    let cutoff = now - chrono::Duration::minutes(age);
    sqlx::query(
        "UPDATE pg_loco_queue SET status=$1,updated_at=$2 WHERE status=$3 AND updated_at <= $4",
    )
    .bind(JobStatus::Queued.to_string())
    .bind(now)
    .bind(JobStatus::Processing.to_string())
    .bind(cutoff.with_timezone(&Utc))
    .execute(pool)
    .await?;
    Ok(())
}
async fn get_jobs(
    pool: &PgPool,
    status: Option<&Vec<JobStatus>>,
    age: Option<i64>,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<Vec<Job>> {
    let now = crate::time::queue_now(timezone).with_timezone(&Utc);
    let mut q = String::from(
        "SELECT id,name,task_data,status,run_at,interval,created_at,updated_at,tags,priority FROM pg_loco_queue WHERE 1=1",
    );
    if status.is_some_and(|s| !s.is_empty()) {
        q.push_str(" AND status IN (");
        q.push_str(
            &(1..=status.unwrap().len())
                .map(|i| format!("${i}"))
                .collect::<Vec<_>>()
                .join(","),
        );
        q.push(')');
    }
    if age.is_some() {
        let n = if status.is_some_and(|s| !s.is_empty()) {
            status.unwrap().len() + 1
        } else {
            1
        };
        q.push_str(&format!(" AND created_at <= ${n}"));
    }
    let mut query = sqlx::query(AssertSqlSafe(q));
    if let Some(values) = status.filter(|s| !s.is_empty()) {
        for v in values {
            query = query.bind(v.to_string());
        }
    }
    if let Some(days) = age {
        query = query.bind(now - chrono::Duration::days(days));
    }
    let rows = query.fetch_all(pool).await?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            row_to_job(row)
                .map_err(|err| {
                    tracing::error!(%err,"invalid queue row skipped");
                    err
                })
                .ok()
        })
        .collect())
}
