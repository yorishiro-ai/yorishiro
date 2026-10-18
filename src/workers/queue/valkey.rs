use std::{collections::HashMap, sync::Arc, time::Duration};

use chrono::Utc;
use loco_rs::prelude::async_trait;
use loco_rs::{
    bgworker::{Job, JobHandler, JobId, JobStatus, QueueProvider},
    config::RedisQueueConfig,
};
use redis::{AsyncCommands, Client, Script, aio::MultiplexedConnection};
use serde_json::Value as JsonValue;
use tokio::sync::watch;

const QUEUE: &str = "queue:";
const JOB: &str = "job:";
const PROCESSING: &str = "processing:";
const FAILED: &str = "failed:";
const CANCELLED: &str = "cancelled:";
const ACQUIRE: &str = "local s=redis.call('ZSCORE',KEYS[1],ARGV[1]); if s then redis.call('ZREM',KEYS[1],ARGV[1]); redis.call('SADD',KEYS[2],ARGV[1]); return s end; return nil";
const WORKER_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const WORKER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct ValkeyTimezoneQueue {
    client: Client,
    handlers: Arc<tokio::sync::Mutex<HashMap<String, Arc<JobHandler>>>>,
    queues: Vec<String>,
    num_workers: u32,
    reaper: Option<loco_rs::config::ReaperConfig>,
    token: watch::Sender<bool>,
    timezone: crate::time::Timezone,
    dangerous_flush: bool,
}

impl ValkeyTimezoneQueue {
    /// Creates a provider using the configured Valkey queue settings.
    ///
    /// # Errors
    /// Returns an error when the Valkey client cannot be configured.
    pub async fn connect(
        config: &RedisQueueConfig,
        timezone: crate::time::Timezone,
    ) -> loco_rs::Result<Self> {
        let client = Client::open(config.uri.as_str()).map_err(loco_rs::Error::from)?;
        Self::from_client(client, config, timezone)
    }

    fn from_client(
        client: Client,
        config: &RedisQueueConfig,
        timezone: crate::time::Timezone,
    ) -> loco_rs::Result<Self> {
        let queues = loco_rs::bgworker::redis::get_queues(&config.queues);
        Ok(Self {
            client,
            handlers: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            queues,
            num_workers: config.num_workers,
            reaper: config.reaper.clone(),
            token: watch::channel(false).0,
            timezone,
            dangerous_flush: config.dangerously_flush,
        })
    }

    async fn connection(&self) -> loco_rs::Result<MultiplexedConnection> {
        self.client
            .get_multiplexed_async_connection()
            .await
            .map_err(loco_rs::Error::from)
    }
}

async fn worker_connection(client: &Client) -> redis::RedisResult<MultiplexedConnection> {
    let config = redis::AsyncConnectionConfig::new()
        .set_connection_timeout(Some(WORKER_CONNECT_TIMEOUT))
        .set_response_timeout(Some(WORKER_RESPONSE_TIMEOUT));
    client
        .get_multiplexed_async_connection_with_config(&config)
        .await
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_with_id(
    client: &Client,
    timezone: crate::time::Timezone,
    class: String,
    queue: Option<String>,
    args: JsonValue,
    tags: Option<Vec<String>>,
    priority: Option<i32>,
    id: String,
) -> loco_rs::Result<Option<String>> {
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .map_err(loco_rs::Error::from)?;
    let queue = queue.unwrap_or_else(|| "default".into());
    let (job, json) = job_json(
        id.clone(),
        class,
        args,
        tags,
        priority.unwrap_or(0),
        timezone,
    )?;
    let enqueue_script = Script::new(
        "if redis.call('EXISTS',KEYS[1])==1 then return 0 end; redis.call('SET',KEYS[1],ARGV[1]); redis.call('ZADD',KEYS[2],ARGV[2],ARGV[3]); return 1",
    );
    let mut invoker = enqueue_script.key(format!("{JOB}{id}"));
    let committed: i32 = invoker
        .key(format!("{QUEUE}{queue}"))
        .arg(json)
        .arg(score(job.priority))
        .arg(&id)
        .invoke_async(&mut conn)
        .await
        .map_err(loco_rs::Error::from)?;
    if committed != 1 {
        return Err(loco_rs::Error::Message(format!(
            "queue ULID collision detected for {id}"
        )));
    }
    Ok(Some(id))
}

fn score(priority: i32) -> f64 {
    -f64::from(priority)
}
fn job_json(
    id: String,
    name: String,
    data: JsonValue,
    tags: Option<Vec<String>>,
    priority: i32,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<(Job, String)> {
    let now = crate::time::queue_now(timezone).with_timezone(&Utc);
    let job = Job {
        id,
        name,
        data,
        status: JobStatus::Queued,
        run_at: now,
        interval: None,
        created_at: Some(now),
        updated_at: Some(now),
        tags,
        priority,
    };
    let json = serde_json::to_string(&job).map_err(loco_rs::Error::from)?;
    Ok((job, json))
}

#[async_trait]
impl QueueProvider for ValkeyTimezoneQueue {
    async fn enqueue(
        &self,
        class: String,
        queue: Option<String>,
        args: JsonValue,
        tags: Option<Vec<String>>,
        priority: Option<i32>,
    ) -> loco_rs::Result<Option<String>> {
        enqueue_with_id(
            &self.client,
            self.timezone,
            class,
            queue,
            args,
            tags,
            priority,
            ulid::Ulid::generate().to_string(),
        )
        .await
    }

    async fn enqueue_batch(
        &self,
        class: String,
        queue: Option<String>,
        jobs: Vec<(JsonValue, Option<i32>)>,
        tags: Option<Vec<String>>,
    ) -> loco_rs::Result<Vec<JobId>> {
        if jobs.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.connection().await?;
        let queue = queue.unwrap_or_else(|| "default".into());
        let queue_key = format!("{QUEUE}{queue}");
        let mut ids = Vec::with_capacity(jobs.len());
        let mut expected = Vec::with_capacity(jobs.len());
        for (data, priority) in jobs {
            let id = ulid::Ulid::generate().to_string();
            let (job, json) = job_json(
                id.clone(),
                class.clone(),
                data,
                tags.clone(),
                priority.unwrap_or(0),
                self.timezone,
            )?;
            expected.push((id.clone(), job, json));
            if ids.contains(&id) {
                return Err(loco_rs::Error::Message(
                    "duplicate ULID generated within batch".into(),
                ));
            }
            ids.push(id);
        }
        let mut unique_ids = std::collections::HashSet::with_capacity(expected.len());
        if ids.iter().any(|id| !unique_ids.insert(id.clone())) {
            return Err(loco_rs::Error::Message(
                "duplicate ULID generated within batch".into(),
            ));
        }
        let batch_script = Script::new(
            "local n=tonumber(ARGV[1]); for i=1,n do if redis.call('EXISTS',ARGV[2+(i-1)*3])==1 then return 0 end end; for i=1,n do local p=2+(i-1)*3; local key=ARGV[p]; local data=ARGV[p+1]; local score=ARGV[p+2]; redis.call('SET',key,data); redis.call('ZADD',KEYS[1],score,string.sub(key,5)) end; return 1",
        );
        let mut invoker = batch_script.key(&queue_key);
        let mut script = invoker.arg(expected.len());
        for (id, job, json) in expected {
            script = script
                .arg(format!("{JOB}{id}"))
                .arg(json)
                .arg(score(job.priority));
        }
        let committed: i32 = script
            .invoke_async(&mut conn)
            .await
            .map_err(loco_rs::Error::from)?;
        if committed != 1 {
            return Err(loco_rs::Error::Message(
                "ULID collision in atomic Valkey batch; no jobs enqueued".into(),
            ));
        }
        Ok(ids)
    }
    async fn register_handler(&self, name: String, handler: JobHandler) -> loco_rs::Result<()> {
        self.handlers.lock().await.insert(name, Arc::new(handler));
        Ok(())
    }
    async fn run(&self, tags: Vec<String>) -> loco_rs::Result<()> {
        let reaper_handle = if let Some(reaper) = self.reaper.clone() {
            let client = self.client.clone();
            let mut token = self.token.subscribe();
            let timezone = self.timezone;
            Some(tokio::spawn(async move {
                loop {
                    tokio::select! {changed=token.changed()=>if changed.is_err()||*token.borrow(){break},()=tokio::time::sleep(Duration::from_secs(reaper.interval_seconds))=>{if let Err(e)=requeue(&client,reaper.age_minutes,timezone).await{tracing::error!(%e,"Valkey reaper failed")}}}
                }
            }))
        } else {
            None
        };
        let token = self.token.subscribe();
        let mut workers = Vec::new();
        for worker in 0..self.num_workers {
            let client = self.client.clone();
            let handlers = self.handlers.clone();
            let queues = self.queues.clone();
            let tags = tags.clone();
            let mut token = token.clone();
            let timezone = self.timezone;
            let poll_interval = Duration::from_secs(1);
            tracing::debug!(worker, queues=?queues, "starting Valkey queue worker");
            workers.push(tokio::spawn(async move {
                while !*token.borrow() {
                    match dequeue(&client, &queues, &tags, timezone, &mut token).await {
                        Ok(Some((job, queue))) => {
                            let handler = handlers.lock().await.get(&job.name).cloned();
                            let Some(handler) = handler else {
                                tracing::error!(worker, job_name=%job.name, "No handler registered for job");
                                tokio::select! {
                                    biased;
                                    changed = token.changed() => if changed.is_err() || *token.borrow() { break; },
                                    () = tokio::time::sleep(poll_interval) => {},
                                }
                                continue;
                            };
                            let result=handler(job.id.clone(), job.data.clone()).await;
                            match result {
                                Ok(()) => if let Err(error) = finish(&client, &job, &queue, timezone).await {
                                    tracing::error!(%error, job_id=%job.id, "Valkey completion failed");
                                },
                                Err(error) => if let Err(failure) = fail(&client, &job, &queue, &error, timezone).await {
                                    tracing::error!(%failure, job_id=%job.id, "Valkey failure update failed");
                                },
                            }
                        }
                        Ok(None) => tokio::select! {
                            biased;
                            changed = token.changed() => if changed.is_err() || *token.borrow() { break; },
                            () = tokio::time::sleep(poll_interval) => {},
                        },
                        Err(_) if *token.borrow() => break,
                        Err(error) => {
                            tracing::error!(%error, "Valkey dequeue failed");
                            tokio::select! {
                                biased;
                                changed = token.changed() => if changed.is_err() || *token.borrow() { break; },
                                () = tokio::time::sleep(Duration::from_secs(1)) => {},
                            }
                        }
                    }
                }
            }));
        }
        for w in workers {
            match w.await {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {
                    tracing::debug!("Valkey worker task cancelled")
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
        Ok(())
    }
    async fn clear(&self) -> loco_rs::Result<()> {
        if self.dangerous_flush {
            tracing::warn!("dangerously_flush is ignored; clearing only Loco queue namespaces");
        }
        let mut c = self.connection().await?;
        for prefix in [QUEUE, JOB, PROCESSING, FAILED, CANCELLED] {
            let mut cursor = 0_u64;
            loop {
                let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(format!("{prefix}*"))
                    .arg("COUNT")
                    .arg(500)
                    .query_async(&mut c)
                    .await
                    .map_err(loco_rs::Error::from)?;
                if !keys.is_empty() {
                    let _: usize = redis::cmd("UNLINK")
                        .arg(keys)
                        .query_async(&mut c)
                        .await
                        .map_err(loco_rs::Error::from)?;
                }
                cursor = next;
                if cursor == 0 {
                    break;
                }
            }
        }
        Ok(())
    }
    async fn ping(&self) -> loco_rs::Result<()> {
        let mut c = self.connection().await?;
        let _: String = redis::cmd("PING")
            .query_async(&mut c)
            .await
            .map_err(loco_rs::Error::from)?;
        Ok(())
    }
    async fn get_jobs(
        &self,
        status: Option<&Vec<JobStatus>>,
        age_days: Option<i64>,
    ) -> loco_rs::Result<Vec<Job>> {
        get_jobs(&self.client, status, age_days, self.timezone).await
    }
    async fn cancel_jobs_by_name(&self, name: &str) -> loco_rs::Result<()> {
        let mut c = self.connection().await?;
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(format!("{QUEUE}*"))
            .query_async(&mut c)
            .await
            .map_err(loco_rs::Error::from)?;
        for q in keys {
            let ids: Vec<String> = c.zrange(&q, 0, -1).await.map_err(loco_rs::Error::from)?;
            for id in ids {
                let k = format!("{JOB}{id}");
                let raw: Option<String> = c.get(&k).await.map_err(loco_rs::Error::from)?;
                if let Some(raw) = raw
                    && let Ok(mut job) = serde_json::from_str::<Job>(&raw)
                    && job.name == name
                    && job.status == JobStatus::Queued
                {
                    let _: () = redis::pipe()
                        .zrem(&q, &id)
                        .ignore()
                        .set(&k, {
                            job.status = JobStatus::Cancelled;
                            job.updated_at =
                                Some(crate::time::queue_now(self.timezone).with_timezone(&Utc));
                            serde_json::to_string(&job).map_err(loco_rs::Error::from)?
                        })
                        .ignore()
                        .sadd(format!("{CANCELLED}{}", q.trim_start_matches(QUEUE)), &id)
                        .ignore()
                        .query_async(&mut c)
                        .await
                        .map_err(loco_rs::Error::from)?;
                }
            }
        }
        Ok(())
    }
    async fn clear_by_status(&self, status: Vec<JobStatus>) -> loco_rs::Result<()> {
        clear_matching(&self.client, Some(&status), None, self.timezone).await
    }
    async fn clear_jobs_older_than(
        &self,
        age_days: i64,
        status: Option<&Vec<JobStatus>>,
    ) -> loco_rs::Result<()> {
        clear_matching(&self.client, status, Some(age_days), self.timezone).await
    }
    async fn retry_failed(&self, id: Option<&str>) -> loco_rs::Result<u64> {
        retry(&self.client, id, self.timezone).await
    }
    async fn requeue(&self, age_minutes: &i64) -> loco_rs::Result<()> {
        requeue(&self.client, *age_minutes, self.timezone).await
    }
    fn describe(&self) -> String {
        "redis queue".into()
    }
    fn shutdown(&self) -> loco_rs::Result<()> {
        let _ = self.token.send(true);
        Ok(())
    }
}

async fn dequeue(
    client: &Client,
    queues: &[String],
    tags: &[String],
    timezone: crate::time::Timezone,
    token: &mut watch::Receiver<bool>,
) -> loco_rs::Result<Option<(Job, String)>> {
    let mut c = connect_worker(client, token).await?;
    for queue in queues {
        let q = format!("{QUEUE}{queue}");
        const BATCH: isize = 50;
        const MAX_SEARCH: isize = 1000;
        let mut offset = 0;
        let mut candidates = Vec::new();
        while offset < MAX_SEARCH {
            let ids: Vec<String> = match c.zrange(&q, offset, offset + BATCH - 1).await {
                Ok(ids) => ids,
                Err(error) => {
                    tracing::error!(%error, "cannot fetch from Valkey queue, reconnecting");
                    c = reconnect_worker(client, token).await?;
                    offset = 0;
                    continue;
                }
            };
            if ids.is_empty() {
                break;
            }
            let mut payloads = redis::pipe();
            for id in &ids {
                payloads.get(format!("{JOB}{id}"));
            }
            let payloads: Vec<Option<String>> = match payloads.query_async(&mut c).await {
                Ok(payloads) => payloads,
                Err(error) => {
                    tracing::error!(%error, "cannot read Valkey jobs, reconnecting");
                    c = reconnect_worker(client, token).await?;
                    offset = 0;
                    continue;
                }
            };
            for (id, raw) in ids.iter().zip(payloads) {
                if *token.borrow() {
                    return Ok(None);
                }
                if let Some(raw) = raw {
                    tracing::debug!(job_id=%id, job_json=%raw, "scanning Valkey queue item");
                    if let Ok(job) = serde_json::from_str::<Job>(&raw) {
                        let tag_match = if tags.is_empty() {
                            job.tags.as_ref().is_none_or(Vec::is_empty)
                        } else {
                            job.tags
                                .as_ref()
                                .is_some_and(|ts| ts.iter().any(|t| tags.contains(t)))
                        };
                        if !tag_match
                            || job.run_at > crate::time::queue_now(timezone).with_timezone(&Utc)
                        {
                            continue;
                        } else {
                            candidates.push((id.clone(), job));
                        }
                    } else {
                        tracing::error!(job_id=%id,"invalid Valkey job JSON; left in queue")
                    }
                } else {
                    tracing::error!(job_id=%id, queue=%queue, "Job data not found; removing stale queue member");
                    let _: usize = c.zrem(&q, id).await.map_err(loco_rs::Error::from)?;
                }
            }
            offset += BATCH;
        }
        candidates.sort_by(|(ia, a), (ib, b)| {
            b.priority
                .cmp(&a.priority)
                .then(a.run_at.cmp(&b.run_at))
                .then(ia.cmp(ib))
        });
        for (id, job) in candidates {
            let p = format!("{PROCESSING}{queue}");
            let claim: Option<f64> = match Script::new(ACQUIRE)
                .key(&q)
                .key(&p)
                .arg(&id)
                .invoke_async(&mut c)
                .await
            {
                Ok(claim) => claim,
                Err(error) => {
                    tracing::error!(%error, "cannot claim Valkey job, reconnecting");
                    c = reconnect_worker(client, token).await?;
                    continue;
                }
            };
            if claim.is_some() {
                return Ok(Some((job, queue.clone())));
            }
        }
    }
    Ok(None)
}

async fn connect_worker(
    client: &Client,
    token: &mut watch::Receiver<bool>,
) -> loco_rs::Result<MultiplexedConnection> {
    let mut backoff = Duration::from_millis(250);
    loop {
        if *token.borrow() {
            return Err(loco_rs::Error::Message(
                "Valkey worker is shutting down".into(),
            ));
        }
        match worker_connection(client).await {
            Ok(connection) => return Ok(connection),
            Err(error) => {
                tracing::error!(%error, "cannot connect to Valkey worker queue; retrying");
                tokio::select! {
                    biased;
                    changed = token.changed() => {
                        if changed.is_err() || *token.borrow() {
                            return Err(loco_rs::Error::Message("Valkey worker is shutting down".into()));
                        }
                    }
                    () = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(Duration::from_secs(5));
            }
        }
    }
}

async fn reconnect_worker(
    client: &Client,
    token: &mut watch::Receiver<bool>,
) -> loco_rs::Result<MultiplexedConnection> {
    connect_worker(client, token).await
}

async fn finish(
    client: &Client,
    job: &Job,
    queue: &str,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<()> {
    let mut c = client
        .get_multiplexed_async_connection()
        .await
        .map_err(loco_rs::Error::from)?;
    let k = format!("{JOB}{}", job.id);
    let p = format!("{PROCESSING}{queue}");
    if let Some(interval) = job.interval {
        let raw: Option<String> = c.get(&k).await.map_err(loco_rs::Error::from)?;
        let Some(raw) = raw else { return Ok(()) };
        let Ok(mut j) = serde_json::from_str::<Job>(&raw) else {
            return Ok(());
        };
        let now = crate::time::queue_now(timezone).with_timezone(&Utc);
        j.status = JobStatus::Queued;
        j.run_at = now + chrono::Duration::milliseconds(interval);
        j.updated_at = Some(now);
        let _: () = redis::pipe()
            .set(&k, serde_json::to_string(&j).map_err(loco_rs::Error::from)?)
            .zadd(format!("{QUEUE}{queue}"), &j.id, score(j.priority))
            .srem(&p, &j.id)
            .query_async(&mut c)
            .await
            .map_err(loco_rs::Error::from)?;
    } else {
        let raw: Option<String> = c.get(&k).await.map_err(loco_rs::Error::from)?;
        let Some(raw) = raw else { return Ok(()) };
        let Ok(mut j) = serde_json::from_str::<Job>(&raw) else {
            return Ok(());
        };
        j.status = JobStatus::Completed;
        j.updated_at = Some(crate::time::queue_now(timezone).with_timezone(&Utc));
        let updated = serde_json::to_string(&j).map_err(loco_rs::Error::from)?;
        let _: () = c.set(&k, updated).await.map_err(loco_rs::Error::from)?;
        let _: usize = c.srem(&p, &j.id).await.map_err(loco_rs::Error::from)?;
    }
    Ok(())
}

async fn clear_older_than(
    client: &Client,
    status: Option<&Vec<JobStatus>>,
    days: i64,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<()> {
    let mut c = client
        .get_multiplexed_async_connection()
        .await
        .map_err(loco_rs::Error::from)?;
    let cutoff =
        crate::time::queue_now(timezone).with_timezone(&Utc) - chrono::Duration::days(days);
    let qkeys: Vec<String> = redis::cmd("KEYS")
        .arg("queue:*")
        .query_async(&mut c)
        .await
        .map_err(loco_rs::Error::from)?;
    let pkeys: Vec<String> = redis::cmd("KEYS")
        .arg("processing:*")
        .query_async(&mut c)
        .await
        .map_err(loco_rs::Error::from)?;
    let job_keys: Vec<String> = redis::cmd("KEYS")
        .arg("job:*")
        .query_async(&mut c)
        .await
        .map_err(loco_rs::Error::from)?;
    for q in &qkeys {
        let ids: Vec<String> = c.zrange(q, 0, -1).await.map_err(loco_rs::Error::from)?;
        for id in ids {
            let key = format!("{JOB}{id}");
            let raw: Option<String> = c.get(&key).await.map_err(loco_rs::Error::from)?;
            if raw
                .as_deref()
                .and_then(|v| serde_json::from_str::<Job>(v).ok())
                .is_some_and(|j| {
                    j.created_at.is_some_and(|created| created < cutoff)
                        && status.is_none_or(|s| s.contains(&j.status))
                })
            {
                let _: usize = c.zrem(q, &id).await.map_err(loco_rs::Error::from)?;
                let _: usize = c.del(key).await.map_err(loco_rs::Error::from)?;
            }
        }
    }
    for p in &pkeys {
        let ids: Vec<String> = c.smembers(p).await.map_err(loco_rs::Error::from)?;
        for id in ids {
            let key = format!("{JOB}{id}");
            let raw: Option<String> = c.get(&key).await.map_err(loco_rs::Error::from)?;
            if let Some(raw) = raw
                && let Ok(mut j) = serde_json::from_str::<Job>(&raw)
            {
                if j.status == JobStatus::Queued {
                    j.status = JobStatus::Processing;
                }
                if j.created_at.is_some_and(|created| created < cutoff)
                    && status.is_none_or(|s| s.contains(&j.status))
                {
                    let _: usize = c.srem(p, &id).await.map_err(loco_rs::Error::from)?;
                    let _: usize = c.del(key).await.map_err(loco_rs::Error::from)?;
                }
            }
        }
    }
    for key in job_keys {
        let raw: Option<String> = c.get(&key).await.map_err(loco_rs::Error::from)?;
        if raw
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Job>(raw).ok())
            .is_some_and(|job| {
                job.created_at.is_some_and(|created| created < cutoff)
                    && status.is_none_or(|statuses| statuses.contains(&job.status))
            })
        {
            let _: usize = c.del(key).await.map_err(loco_rs::Error::from)?;
        }
    }
    Ok(())
}

async fn fail(
    client: &Client,
    job: &Job,
    queue: &str,
    error: &loco_rs::Error,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<()> {
    let mut c = client
        .get_multiplexed_async_connection()
        .await
        .map_err(loco_rs::Error::from)?;
    let key = format!("{JOB}{}", job.id);
    let processing = format!("{PROCESSING}{queue}");
    let raw: Option<String> = c.get(&key).await.map_err(loco_rs::Error::from)?;
    let Some(raw) = raw else {
        let _: () = c
            .srem(processing, &job.id)
            .await
            .map_err(loco_rs::Error::from)?;
        return Ok(());
    };
    let Ok(mut j) = serde_json::from_str::<Job>(&raw) else {
        let _: () = c
            .srem(processing, &job.id)
            .await
            .map_err(loco_rs::Error::from)?;
        return Ok(());
    };
    if let Some(o) = j.data.as_object_mut() {
        o.insert("error".into(), JsonValue::String(error.to_string()));
    } else {
        j.data = serde_json::json!({"args":j.data,"error":error.to_string()});
    }
    j.status = JobStatus::Failed;
    j.updated_at = Some(crate::time::queue_now(timezone).with_timezone(&Utc));
    let _: () = redis::pipe()
        .set(
            key,
            serde_json::to_string(&j).map_err(loco_rs::Error::from)?,
        )
        .ignore()
        .srem(processing, &j.id)
        .ignore()
        .query_async(&mut c)
        .await
        .map_err(loco_rs::Error::from)?;
    Ok(())
}

async fn get_jobs(
    client: &Client,
    status: Option<&Vec<JobStatus>>,
    age: Option<i64>,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<Vec<Job>> {
    let mut c = client
        .get_multiplexed_async_connection()
        .await
        .map_err(loco_rs::Error::from)?;
    let now = crate::time::queue_now(timezone).with_timezone(&Utc);
    let pkeys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{PROCESSING}*"))
        .query_async(&mut c)
        .await
        .map_err(loco_rs::Error::from)?;
    let mut processing = std::collections::HashSet::new();
    for key in pkeys {
        let ids: Vec<String> = c.smembers(key).await.map_err(loco_rs::Error::from)?;
        processing.extend(ids);
    }
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{JOB}*"))
        .query_async(&mut c)
        .await
        .map_err(loco_rs::Error::from)?;
    let cutoff = age.map(|days| now - chrono::Duration::days(days));
    let mut jobs = Vec::new();
    for key in keys {
        if let Some(raw) = c
            .get::<_, Option<String>>(key)
            .await
            .map_err(loco_rs::Error::from)?
            && let Ok(job) = serde_json::from_str::<Job>(&raw)
        {
            let effective_status =
                if job.status == JobStatus::Queued && processing.contains(&job.id) {
                    JobStatus::Processing
                } else {
                    job.status.clone()
                };
            if status.is_none_or(|s| s.contains(&effective_status))
                && cutoff.is_none_or(|cut| job.created_at.is_none_or(|created| created <= cut))
            {
                jobs.push(job);
            }
        }
    }
    Ok(jobs)
}

async fn clear_matching(
    client: &Client,
    status: Option<&Vec<JobStatus>>,
    age: Option<i64>,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<()> {
    if let Some(days) = age {
        return clear_older_than(client, status, days, timezone).await;
    }
    let mut c = client
        .get_multiplexed_async_connection()
        .await
        .map_err(loco_rs::Error::from)?;
    let queue_keys: Vec<String> = redis::cmd("KEYS")
        .arg("queue:*")
        .query_async(&mut c)
        .await
        .map_err(loco_rs::Error::from)?;
    let processing_keys: Vec<String> = redis::cmd("KEYS")
        .arg("processing:*")
        .query_async(&mut c)
        .await
        .map_err(loco_rs::Error::from)?;
    let mut memberships = std::collections::HashMap::<String, Vec<String>>::new();
    for key in &queue_keys {
        memberships.insert(
            key.clone(),
            c.zrange(key, 0, -1).await.map_err(loco_rs::Error::from)?,
        );
    }
    for key in &processing_keys {
        memberships.insert(
            key.clone(),
            c.smembers(key).await.map_err(loco_rs::Error::from)?,
        );
    }
    if let Some(statuses) = status.filter(|statuses| !statuses.is_empty()) {
        for (membership, ids) in memberships.iter().filter(|(key, _)| key.starts_with(QUEUE)) {
            for id in ids {
                let key = format!("{JOB}{id}");
                let raw: Option<String> = c.get(&key).await.map_err(loco_rs::Error::from)?;
                if raw
                    .as_deref()
                    .and_then(|raw| serde_json::from_str::<Job>(raw).ok())
                    .is_some_and(|job| statuses.contains(&job.status))
                {
                    let _: usize = c.zrem(membership, id).await.map_err(loco_rs::Error::from)?;
                    let _: usize = c.del(key).await.map_err(loco_rs::Error::from)?;
                }
            }
        }
        for (membership, ids) in memberships
            .iter()
            .filter(|(key, _)| key.starts_with(PROCESSING))
        {
            for id in ids {
                let key = format!("{JOB}{id}");
                let raw: Option<String> = c.get(&key).await.map_err(loco_rs::Error::from)?;
                if let Some(raw) = raw
                    && let Ok(mut job) = serde_json::from_str::<Job>(&raw)
                {
                    if job.status == JobStatus::Queued {
                        job.status = JobStatus::Processing;
                    }
                    if statuses.contains(&job.status) {
                        let _: usize =
                            c.srem(membership, id).await.map_err(loco_rs::Error::from)?;
                        let _: usize = c.del(key).await.map_err(loco_rs::Error::from)?;
                    }
                }
            }
        }
        let job_keys: Vec<String> = redis::cmd("KEYS")
            .arg("job:*")
            .query_async(&mut c)
            .await
            .map_err(loco_rs::Error::from)?;
        for key in job_keys {
            let raw: Option<String> = c.get(&key).await.map_err(loco_rs::Error::from)?;
            if raw
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Job>(raw).ok())
                .is_some_and(|job| statuses.contains(&job.status))
            {
                let _: usize = c.del(key).await.map_err(loco_rs::Error::from)?;
            }
        }
    }
    Ok(())
}
async fn retry(
    client: &Client,
    id: Option<&str>,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<u64> {
    let jobs = get_jobs(client, Some(&vec![JobStatus::Failed]), None, timezone).await?;
    let mut c = client
        .get_multiplexed_async_connection()
        .await
        .map_err(loco_rs::Error::from)?;
    let mut count = 0;
    for mut j in jobs {
        if id.is_some_and(|id| id != j.id) || j.status != JobStatus::Failed {
            continue;
        }
        j.status = JobStatus::Queued;
        j.run_at = crate::time::queue_now(timezone).with_timezone(&Utc);
        j.updated_at = Some(j.run_at);
        let job_json = serde_json::to_string(&j).map_err(loco_rs::Error::from)?;
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.set(format!("{JOB}{}", j.id), job_json).ignore();
        pipe.zadd(format!("{QUEUE}default"), &j.id, score(j.priority))
            .ignore();
        let _: () = pipe
            .query_async(&mut c)
            .await
            .map_err(loco_rs::Error::from)?;
        count += 1;
    }
    Ok(count)
}

async fn requeue(
    client: &Client,
    age: i64,
    timezone: crate::time::Timezone,
) -> loco_rs::Result<()> {
    let mut c = client
        .get_multiplexed_async_connection()
        .await
        .map_err(loco_rs::Error::from)?;
    let pkeys: Vec<String> = redis::cmd("KEYS")
        .arg("processing:*")
        .query_async(&mut c)
        .await
        .map_err(loco_rs::Error::from)?;
    let cutoff =
        crate::time::queue_now(timezone).with_timezone(&Utc) - chrono::Duration::minutes(age);
    for p in pkeys {
        let qname = p.trim_start_matches(PROCESSING);
        let ids: Vec<String> = c.smembers(&p).await.map_err(loco_rs::Error::from)?;
        for id in ids {
            let k = format!("{JOB}{id}");
            let raw: Option<String> = c.get(&k).await.map_err(loco_rs::Error::from)?;
            if let Some(raw) = raw
                && let Ok(mut j) = serde_json::from_str::<Job>(&raw)
            {
                let stale = j.updated_at.is_some_and(|updated| updated < cutoff)
                    || (j.updated_at.is_none()
                        && j.created_at.is_some_and(|created| created < cutoff));
                if stale {
                    j.status = JobStatus::Queued;
                    j.updated_at = Some(crate::time::queue_now(timezone).with_timezone(&Utc));
                    let script = Script::new(
                        "if redis.call('SISMEMBER',KEYS[1],ARGV[1])==0 then return 0 end; redis.call('SREM',KEYS[1],ARGV[1]); redis.call('SET',KEYS[2],ARGV[2]); redis.call('ZADD',KEYS[3],ARGV[3],ARGV[1]); return 1",
                    );
                    let mut invocation = script.key(&p);
                    let result: i32 = invocation
                        .key(&k)
                        .key(format!("{QUEUE}{qname}"))
                        .arg(&id)
                        .arg(serde_json::to_string(&j).map_err(loco_rs::Error::from)?)
                        .arg(score(j.priority))
                        .invoke_async(&mut c)
                        .await
                        .map_err(loco_rs::Error::from)?;
                    if result == 0 {
                        continue;
                    }
                }
            }
        }
    }
    let failed_keys: Vec<String> = redis::cmd("KEYS")
        .arg("failed:*")
        .query_async(&mut c)
        .await
        .map_err(loco_rs::Error::from)?;
    for failed_key in failed_keys {
        let queue_name = failed_key.trim_start_matches(FAILED);
        let queue_key = format!("{QUEUE}{queue_name}");
        let ids: Vec<String> = c
            .smembers(&failed_key)
            .await
            .map_err(loco_rs::Error::from)?;
        for id in ids {
            let key = format!("{JOB}{id}");
            let raw: Option<String> = c.get(&key).await.map_err(loco_rs::Error::from)?;
            if let Some(raw) = raw
                && let Ok(mut job) = serde_json::from_str::<Job>(&raw)
                && job
                    .updated_at
                    .is_some_and(|updated| updated < cutoff && job.status == JobStatus::Failed)
            {
                job.status = JobStatus::Queued;
                job.updated_at = Some(crate::time::queue_now(timezone).with_timezone(&Utc));
                let script = Script::new(
                    "if redis.call('SISMEMBER',KEYS[1],ARGV[1])==0 then return 0 end; redis.call('SREM',KEYS[1],ARGV[1]); redis.call('SET',KEYS[2],ARGV[2]); redis.call('ZADD',KEYS[3],ARGV[3],ARGV[1]); return 1",
                );
                let mut invocation = script.key(&failed_key);
                let _: i32 = invocation
                    .key(&key)
                    .key(&queue_key)
                    .arg(&id)
                    .arg(serde_json::to_string(&job).map_err(loco_rs::Error::from)?)
                    .arg(score(job.priority))
                    .invoke_async(&mut c)
                    .await
                    .map_err(loco_rs::Error::from)?;
            }
        }
    }
    Ok(())
}
