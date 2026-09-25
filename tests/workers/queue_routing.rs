//! Regression coverage for Loco queue tag routing through its public `Queue` API.
//!
//! A worker started without tags must leave tagged jobs alone.
//! A worker started with `worker-class:shared` must dequeue the same job.
//! The tests exercise both SQL providers without copying their dequeue SQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use loco_rs::bgworker::{self, BackgroundWorker};
use loco_rs::config::{PostgresQueueConfig, SqliteQueueConfig};
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

async fn empty_tags_leave_the_probe_queued(queue: Arc<bgworker::Queue>) {
    DEQUEUE_HITS.store(0, Ordering::SeqCst);
    queue
        .register(RoutingProbeWorker)
        .await
        .expect("register probe");
    enqueue_tagged_probe(&queue).await;

    let running_queue = queue.clone();
    let handle = tokio::spawn(async move { running_queue.run(vec![]).await });
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    assert_eq!(
        DEQUEUE_HITS.load(Ordering::SeqCst),
        0,
        "empty tags consumed a tagged job"
    );
    queue.shutdown().expect("stop empty-tag worker");
    handle
        .await
        .expect("join empty-tag worker")
        .expect("run empty-tag worker");
}

async fn shared_tag_dequeues_the_probe(queue: Arc<bgworker::Queue>) {
    queue
        .register(RoutingProbeWorker)
        .await
        .expect("register probe");

    let running_queue = queue.clone();
    let handle = tokio::spawn(async move { running_queue.run(vec![SHARED_TAG.to_string()]).await });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while DEQUEUE_HITS.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("shared-tag worker dequeues tagged job");
    queue.shutdown().expect("stop shared-tag worker");
    handle
        .await
        .expect("join shared-tag worker")
        .expect("run shared-tag worker");
}

#[tokio::test]
#[serial(queue_postgres)]
async fn postgres_queue_empty_tags_exclude_tagged_jobs_and_shared_tag_dequeues_them() {
    if !crate::require_postgres_backend() {
        return;
    }

    let uri = std::env::var("DATABASE_URL").expect("PostgreSQL DATABASE_URL");
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
    let empty_queue = Arc::new(
        bgworker::pg::create_provider(&config)
            .await
            .expect("Postgres queue"),
    );
    empty_queue.setup().await.expect("set up Postgres queue");
    empty_tags_leave_the_probe_queued(empty_queue).await;

    let shared_queue = Arc::new(
        bgworker::pg::create_provider(&config)
            .await
            .expect("Postgres queue"),
    );
    shared_queue.setup().await.expect("set up Postgres queue");
    shared_tag_dequeues_the_probe(shared_queue).await;
}

#[tokio::test]
#[serial(queue_postgres)]
async fn sqlite_queue_empty_tags_exclude_tagged_jobs_and_shared_tag_dequeues_them() {
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
    let empty_queue = Arc::new(
        bgworker::sqlt::create_provider(&config)
            .await
            .expect("SQLite queue"),
    );
    empty_queue.setup().await.expect("set up SQLite queue");
    empty_tags_leave_the_probe_queued(empty_queue).await;

    let shared_queue = Arc::new(
        bgworker::sqlt::create_provider(&config)
            .await
            .expect("SQLite queue"),
    );
    shared_queue.setup().await.expect("set up SQLite queue");
    shared_tag_dequeues_the_probe(shared_queue).await;
}
