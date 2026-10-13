//! A worker started with one target tag takes only the jobs of that target, on every queue provider.
//!
//! The tag is a single composite value, so a worker that names `worker-target:shared` can never match a job carrying `worker-target:tenant:<id>`, and neither matches a job still carrying a retired `worker-class:*` tag.
//! Every provider matches a worker's tags against a job's tags with OR, so two separate tags (class and target) would match on either one; one composite tag cannot.
//! PostgreSQL and Redis run in the lanes whose queue they are; SQLite runs everywhere.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::FutureExt;
use loco_rs::bgworker::{self, BackgroundWorker};
use loco_rs::config::{PostgresQueueConfig, RedisQueueConfig, SqliteQueueConfig};
use loco_rs::prelude::*;
use serial_test::serial;
use yorishiro::workers::target::WorkerTarget;

const SHARED: &str = "worker-target:shared";
const OFFICIAL: &str = "worker-target:official";
const TENANT_A: &str = "worker-target:tenant:0198f3a0-0000-7000-8000-00000000000a";
const TENANT_B: &str = "worker-target:tenant:0198f3a0-0000-7000-8000-00000000000b";
const RETIRED: &str = "worker-class:shared";

/// The tags below are what a production dispatch writes, not a shape this file invented.
#[test]
fn the_probe_tags_are_the_tags_a_dispatch_writes() {
    assert_eq!(SHARED, WorkerTarget::Shared.tag());
    assert_eq!(OFFICIAL, WorkerTarget::Official.tag());
    assert_eq!(TENANT_A, WorkerTarget::parse_tag(TENANT_A).unwrap().tag());
    assert_eq!(TENANT_B, WorkerTarget::parse_tag(TENANT_B).unwrap().tag());
}

static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct TargetProbeWorker;

#[async_trait]
impl BackgroundWorker<serde_json::Value> for TargetProbeWorker {
    fn build(_ctx: &AppContext) -> Self {
        Self
    }

    async fn perform(&self, args: serde_json::Value) -> loco_rs::Result<()> {
        let tag = args["tag"].as_str().unwrap_or_default().to_owned();
        SEEN.lock().expect("seen lock").push(tag);
        Ok(())
    }
}

fn seen() -> Vec<String> {
    let mut seen = SEEN.lock().expect("seen lock").clone();
    seen.sort();
    seen
}

type MakeQueue = Box<dyn Fn() -> Pin<Box<dyn Future<Output = Arc<bgworker::Queue>>>>>;

/// How a job for `tag` is routed on a provider that honours a named queue: the Redis lane enqueues each target to its own queue, the SQL providers ignore the name.
fn queue_for(named_queues: bool, tag: &str) -> Option<String> {
    named_queues.then(|| tag.to_owned())
}

async fn enqueue(queue: &bgworker::Queue, tag: &str, named_queues: bool) {
    queue
        .enqueue(
            TargetProbeWorker::class_name(),
            queue_for(named_queues, tag),
            serde_json::json!({ "tag": tag }),
            Some(vec![tag.to_owned()]),
            None,
        )
        .await
        .expect("enqueue a target job");
}

async fn run_worker_for(
    queue: Arc<bgworker::Queue>,
    tags: Vec<&str>,
    expect: Vec<&str>,
    settle: Duration,
) {
    SEEN.lock().expect("seen lock").clear();
    queue
        .register(TargetProbeWorker)
        .await
        .expect("register probe");
    let running = queue.clone();
    let tags: Vec<String> = tags.into_iter().map(str::to_owned).collect();
    let handle = tokio::spawn(async move { running.run(tags).await });
    let expected = expect.len();
    tokio::time::timeout(Duration::from_secs(10), async {
        while SEEN.lock().expect("seen lock").len() < expected {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the worker takes the jobs of its own target");
    // Give a worker that matches too much time to take what is not its own.
    tokio::time::sleep(settle).await;
    let shutdown = queue.shutdown();
    let joined = handle.await;
    shutdown.expect("stop the worker");
    joined.expect("join the worker").expect("run the worker");
    let mut expect: Vec<String> = expect.into_iter().map(str::to_owned).collect();
    expect.sort();
    assert_eq!(seen(), expect);
}

/// Four targets and a retired class tag are queued on one provider, then workers for one target at a time drain it.
async fn each_worker_takes_only_its_own_target(
    first: Arc<bgworker::Queue>,
    next: MakeQueue,
    named_queues: bool,
) {
    let settle = Duration::from_millis(2500);
    for tag in [SHARED, OFFICIAL, TENANT_A, TENANT_B, RETIRED] {
        enqueue(&first, tag, named_queues).await;
    }

    run_worker_for(first.clone(), vec![TENANT_A], vec![TENANT_A], settle).await;
    run_worker_for(next().await, vec![SHARED], vec![SHARED], settle).await;
    run_worker_for(next().await, vec![OFFICIAL], vec![OFFICIAL], settle).await;
    // One process may serve several targets, and then takes exactly those.
    run_worker_for(
        next().await,
        vec![TENANT_B, OFFICIAL],
        vec![TENANT_B],
        settle,
    )
    .await;
    // The retired class tag matches no target, so nothing is left for a target worker to take.
    run_worker_for(
        next().await,
        vec![SHARED, OFFICIAL, TENANT_A, TENANT_B],
        vec![],
        settle,
    )
    .await;
}

fn sqlite_config(path: &std::path::Path) -> SqliteQueueConfig {
    SqliteQueueConfig {
        uri: format!("sqlite://{}?mode=rwc", path.display()),
        dangerously_flush: false,
        enable_logging: false,
        max_connections: 2,
        min_connections: 1,
        connect_timeout: 5_000,
        idle_timeout: 5_000,
        poll_interval_sec: 1,
        num_workers: 1,
        reaper: None,
    }
}

#[tokio::test]
#[serial(process_environment)]
async fn sqlite_queue_delivers_each_target_only_to_its_worker() {
    let directory = tempfile::tempdir().expect("queue tempdir");
    let path = directory.path().join("queue.sqlite3");
    let make = {
        let path = path.clone();
        move || {
            let config = sqlite_config(&path);
            Box::pin(async move {
                let queue = Arc::new(
                    bgworker::sqlt::create_provider(&config)
                        .await
                        .expect("SQLite queue"),
                );
                queue.setup().await.expect("set up SQLite queue");
                queue
            }) as Pin<Box<dyn Future<Output = Arc<bgworker::Queue>>>>
        }
    };
    let first = make().await;
    each_worker_takes_only_its_own_target(first, Box::new(make), false).await;
}

#[tokio::test]
#[serial(queue_postgres)]
#[serial(process_environment)]
async fn postgres_queue_delivers_each_target_only_to_its_worker() {
    let is_postgres =
        |url: &String| url.starts_with("postgres://") || url.starts_with("postgresql://");
    let Some(uri) = std::env::var("QUEUE_URL")
        .ok()
        .filter(is_postgres)
        .or_else(|| std::env::var("DATABASE_URL").ok().filter(is_postgres))
    else {
        return;
    };
    let make = move || {
        let uri = uri.clone();
        Box::pin(async move {
            let config = PostgresQueueConfig {
                uri,
                dangerously_flush: false,
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
            queue.setup().await.expect("set up Postgres queue");
            queue
        }) as Pin<Box<dyn Future<Output = Arc<bgworker::Queue>>>>
    };
    let first = make().await;
    first.clear().await.expect("empty the Postgres queue");
    let outcome = std::panic::AssertUnwindSafe(each_worker_takes_only_its_own_target(
        first.clone(),
        Box::new(make),
        false,
    ))
    .catch_unwind()
    .await;
    let _ = first.clear().await;
    let _ = first.shutdown();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[serial(queue_postgres)]
#[serial(process_environment)]
async fn redis_queue_delivers_each_target_only_to_its_worker() {
    redis_scenario(false).await;
}

#[tokio::test]
#[serial(queue_postgres)]
#[serial(process_environment)]
async fn redis_queue_with_one_named_queue_per_target_delivers_only_to_its_worker() {
    redis_scenario(true).await;
}

async fn redis_scenario(named_queues: bool) {
    let Some(uri) = crate::valkey_test_url() else {
        return;
    };
    let make = move || {
        let uri = uri.clone();
        Box::pin(async move {
            // A worker polls only the queues its configuration names: the shared default queue, or the one queue per target it serves.
            let queues = named_queues.then(|| {
                vec![
                    SHARED.to_owned(),
                    OFFICIAL.to_owned(),
                    TENANT_A.to_owned(),
                    TENANT_B.to_owned(),
                    RETIRED.to_owned(),
                ]
            });
            let config = RedisQueueConfig {
                uri,
                dangerously_flush: false,
                queues,
                num_workers: 1,
                reaper: None,
            };
            let queue = Arc::new(
                bgworker::redis::create_provider(&config)
                    .await
                    .expect("Redis queue"),
            );
            queue.setup().await.expect("set up Redis queue");
            queue
        }) as Pin<Box<dyn Future<Output = Arc<bgworker::Queue>>>>
    };
    let first = make().await;
    first
        .clear()
        .await
        .expect("clear the reserved Redis database");
    let outcome = std::panic::AssertUnwindSafe(each_worker_takes_only_its_own_target(
        first.clone(),
        Box::new(make),
        named_queues,
    ))
    .catch_unwind()
    .await;
    let _ = first.clear().await;
    let _ = first.shutdown();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
