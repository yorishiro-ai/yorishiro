use std::{sync::Arc, time::Duration};

use chrono::Utc;
use loco_rs::prelude::async_trait;
use loco_rs::{
    bgworker::{Job, JobHandler, JobId, JobStatus, QueueProvider},
    config::PostgresQueueConfig,
};
use sea_orm::sqlx::{
    self, ConnectOptions, Executor, PgPool, Postgres, QueryBuilder, postgres::PgConnectOptions,
};
use sea_orm::{
    ConnectionTrait, ExprTrait,
    sea_query::{Alias, Expr, OnConflict, Query},
};
use serde_json::Value as JsonValue;

#[derive(Clone)]
pub struct PostgresTimezoneQueue {
    pub pool: PgPool,
    worker: Arc<loco_rs::bgworker::pg::PgQueue>,
    timezone: crate::time::Timezone,
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
            worker: Arc::new(loco_rs::bgworker::pg::PgQueue {
                pool: pool.clone(),
                registry: Arc::new(tokio::sync::Mutex::new(
                    loco_rs::bgworker::pg::JobRegistry::new(),
                )),
                run_opts: loco_rs::bgworker::pg::RunOpts {
                    num_workers: config.num_workers,
                    poll_interval_sec: config.poll_interval_sec,
                    reaper: config.reaper.clone(),
                },
                token: Default::default(),
            }),
            pool,
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
        let statement = Query::insert()
            .into_table(Alias::new("pg_loco_queue"))
            .columns(
                [
                    "id",
                    "task_data",
                    "name",
                    "run_at",
                    "interval",
                    "tags",
                    "priority",
                    "created_at",
                    "updated_at",
                ]
                .map(Alias::new),
            )
            .values_panic([
                id.clone().into(),
                args.into(),
                class.into(),
                now.into(),
                Option::<i64>::None.into(),
                tags.map(serde_json::to_value).transpose()?.into(),
                priority.unwrap_or(0).into(),
                now.into(),
                now.into(),
            ])
            .on_conflict(OnConflict::column(Alias::new("id")).do_nothing().to_owned())
            .to_owned();
        let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(self.pool.clone());
        let inserted = db
            .execute_raw(db.get_database_backend().build(&statement))
            .await?;
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
        let id = ulid::Ulid::generate().to_string();
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
            let id = ulid::Ulid::generate().to_string();
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
        self.worker.register_handler(name, handler).await
    }
    async fn run(&self, tags: Vec<String>) -> loco_rs::Result<()> {
        self.worker.run(tags).await
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
        self.worker
            .get_jobs(status.filter(|values| !values.is_empty()), age_days)
            .await
    }
    async fn cancel_jobs_by_name(&self, name: &str) -> loco_rs::Result<()> {
        self.worker.cancel_jobs_by_name(name).await
    }
    async fn clear_by_status(&self, status: Vec<JobStatus>) -> loco_rs::Result<()> {
        if status.is_empty() {
            return Ok(());
        }
        self.worker.clear_by_status(status).await
    }
    async fn clear_jobs_older_than(
        &self,
        age_days: i64,
        status: Option<&Vec<JobStatus>>,
    ) -> loco_rs::Result<()> {
        let cutoff = crate::time::queue_now(self.timezone).with_timezone(&Utc)
            - chrono::Duration::days(age_days);
        let mut statement = Query::delete()
            .from_table(Alias::new("pg_loco_queue"))
            .and_where(Expr::col(Alias::new("created_at")).lte(cutoff))
            .to_owned();
        if let Some(values) = status.filter(|values| !values.is_empty()) {
            statement.and_where(
                Expr::col(Alias::new("status")).is_in(values.iter().map(ToString::to_string)),
            );
        }
        let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(self.pool.clone());
        db.execute_raw(db.get_database_backend().build(&statement))
            .await?;
        Ok(())
    }
    async fn retry_failed(&self, id: Option<&str>) -> loco_rs::Result<u64> {
        self.worker.retry_failed(id).await
    }

    async fn requeue(&self, age_minutes: &i64) -> loco_rs::Result<()> {
        self.worker.requeue(age_minutes).await
    }
    fn describe(&self) -> String {
        "postgres queue".into()
    }
    fn shutdown(&self) -> loco_rs::Result<()> {
        self.worker.shutdown()
    }
}
