//! Regression coverage for Loco queue tag routing through its public `Queue` API.
//!
//! A worker started with `worker-class:shared` must dequeue a tagged job.
//! The tests exercise all three providers (SQLite, PostgreSQL and Redis-compatible) without copying their dequeue logic; PostgreSQL and Redis run in the lanes whose queue they are.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::FutureExt;
use loco_rs::bgworker::{self, BackgroundWorker};
use loco_rs::config::{PostgresQueueConfig, RedisQueueConfig, SqliteQueueConfig};
use loco_rs::prelude::*;
use serial_test::serial;

const SHARED_TAG: &str = "worker-class:shared";
static DEQUEUE_HITS: AtomicUsize = AtomicUsize::new(0);

struct RoutingProbeWorker;

#[async_trait]
impl BackgroundWorker<serde_json::Value> for RoutingProbeWorker {
    fn build(_ctx: &AppContext) -> Self {
        Self
    }

    fn tags() -> Vec<String> {
        vec![SHARED_TAG.to_string()]
    }

    async fn perform(&self, _args: serde_json::Value) -> loco_rs::Result<()> {
        DEQUEUE_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

async fn enqueue_tagged_probe(queue: &bgworker::Queue) {
    queue
        .enqueue(
            RoutingProbeWorker::class_name(),
            None,
            serde_json::json!({}),
            Some(vec![SHARED_TAG.to_string()]),
            None,
        )
        .await
        .expect("enqueue tagged probe");
}

async fn shared_tag_dequeues_the_probe(queue: Arc<bgworker::Queue>) {
    DEQUEUE_HITS.store(0, Ordering::SeqCst);
    queue
        .register(RoutingProbeWorker)
        .await
        .expect("register probe");
    enqueue_tagged_probe(&queue).await;

    let running_queue = queue.clone();
    let handle = tokio::spawn(async move { running_queue.run(vec![SHARED_TAG.to_string()]).await });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while DEQUEUE_HITS.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("shared-tag worker dequeues tagged job");
    let shutdown = queue.shutdown();
    let joined = handle.await;
    shutdown.expect("stop shared-tag worker");
    joined
        .expect("join shared-tag worker")
        .expect("run shared-tag worker");
    assert_eq!(DEQUEUE_HITS.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[serial(queue_postgres)]
#[serial(process_environment)]
async fn postgres_queue_routes_tagged_jobs_to_matching_workers() {
    let is_postgres =
        |url: &String| url.starts_with("postgres://") || url.starts_with("postgresql://");
    let Some(uri) = std::env::var("QUEUE_URL")
        .ok()
        .filter(is_postgres)
        .or_else(|| std::env::var("DATABASE_URL").ok().filter(is_postgres))
    else {
        return;
    };
    let config = PostgresQueueConfig {
        uri,
        dangerously_flush: true,
        enable_logging: false,
        max_connections: 2,
        min_connections: 1,
        connect_timeout: 5_000,
        idle_timeout: 5_000,
        poll_interval_sec: 1,
        num_workers: 1,
        reaper: None,
    };
    let queue = Arc::new(
        bgworker::pg::create_provider(&config)
            .await
            .expect("Postgres queue"),
    );
    let result = std::panic::AssertUnwindSafe(async {
        queue.setup().await.expect("set up Postgres queue");
        shared_tag_dequeues_the_probe(queue.clone()).await;
    })
    .catch_unwind()
    .await;
    let _ = queue.shutdown();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[serial(queue_postgres)]
#[serial(process_environment)]
async fn sqlite_queue_routes_tagged_jobs_to_matching_workers() {
    let directory = tempfile::tempdir().expect("queue tempdir");
    let uri = format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("queue.sqlite3").display()
    );
    let config = SqliteQueueConfig {
        uri,
        dangerously_flush: true,
        enable_logging: false,
        max_connections: 2,
        min_connections: 1,
        connect_timeout: 5_000,
        idle_timeout: 5_000,
        poll_interval_sec: 1,
        num_workers: 1,
        reaper: None,
    };
    let queue = Arc::new(
        bgworker::sqlt::create_provider(&config)
            .await
            .expect("SQLite queue"),
    );
    let result = std::panic::AssertUnwindSafe(async {
        queue.setup().await.expect("set up SQLite queue");
        shared_tag_dequeues_the_probe(queue.clone()).await;
    })
    .catch_unwind()
    .await;
    let _ = queue.shutdown();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[serial(queue_postgres)]
#[serial(process_environment)]
async fn redis_queue_routes_tagged_jobs_to_matching_workers() {
    let Some(uri) = crate::valkey_test_url() else {
        return;
    };
    let config = RedisQueueConfig {
        uri,
        dangerously_flush: true,
        queues: None,
        num_workers: 1,
        reaper: None,
    };
    let queue = Arc::new(
        bgworker::redis::create_provider(&config)
            .await
            .expect("Redis queue"),
    );
    let result = std::panic::AssertUnwindSafe(async {
        queue.setup().await.expect("set up Redis queue");
        queue
            .clear()
            .await
            .expect("clear the reserved Redis database");
        shared_tag_dequeues_the_probe(queue.clone()).await;
    })
    .catch_unwind()
    .await;
    let _ = queue.clear().await;
    let _ = queue.shutdown();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
