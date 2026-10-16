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

type OwnedRedisJobs = Arc<Mutex<Vec<(String, String)>>>;

/// How a job for `tag` is routed on a provider that honours a named queue: the Redis lane enqueues each target to its own queue, the SQL providers ignore the name.
fn queue_for(named_queues: bool, tag: &str) -> Option<String> {
    named_queues.then(|| tag.to_owned())
}

async fn enqueue(queue: &bgworker::Queue, tag: &str, queue_name: Option<String>) -> String {
    queue
        .enqueue(
            TargetProbeWorker::class_name(),
            queue_name,
            serde_json::json!({ "tag": tag }),
            Some(vec![tag.to_owned()]),
            None,
        )
        .await
        .expect("enqueue a target job")
        .expect("queue provider returns a job ID")
}

async fn run_worker_for(
    queue: Arc<bgworker::Queue>,
    tags: Vec<String>,
    expect: Vec<String>,
    settle: Duration,
) {
    SEEN.lock().expect("seen lock").clear();
    queue
        .register(TargetProbeWorker)
        .await
        .expect("register probe");
    let running = queue.clone();
    let handle = tokio::spawn(async move { running.run(tags).await });
    let expected = expect.len();
    let processed = tokio::time::timeout(Duration::from_secs(10), async {
        while SEEN.lock().expect("seen lock").len() < expected {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    // Give a worker that matches too much time to take what is not its own.
    if processed.is_ok() {
        tokio::time::sleep(settle).await;
    }
    let shutdown = queue.shutdown();
    let joined = handle.await;
    shutdown.expect("stop the worker");
    joined.expect("join the worker").expect("run the worker");
    processed.expect("the worker takes the jobs of its own target");
    let mut expect = expect;
    expect.sort();
    assert_eq!(seen(), expect);
}

/// Four targets and a retired class tag are queued on one provider, then workers for one target at a time drain it.
async fn each_worker_takes_only_its_own_target(
    first: Arc<bgworker::Queue>,
    next: MakeQueue,
    named_queues: bool,
    redis_routes: Option<Vec<(String, Option<String>)>>,
    owned_redis_jobs: Option<OwnedRedisJobs>,
) {
    let settle = Duration::from_millis(2500);
    let routes = redis_routes.unwrap_or_else(|| {
        [SHARED, OFFICIAL, TENANT_A, TENANT_B, RETIRED]
            .into_iter()
            .map(|tag| (tag.to_owned(), queue_for(named_queues, tag)))
            .collect()
    });
    assert_eq!(routes.len(), 5);
    for (tag, queue_name) in &routes {
        let queue_member = queue_name.clone().unwrap_or_else(|| "default".into());
        let id = enqueue(&first, tag, queue_name.clone()).await;
        if let Some(owned_ids) = &owned_redis_jobs {
            owned_ids
                .lock()
                .expect("owned Redis jobs")
                .push((queue_member, id));
        }
    }

    let tags: Vec<String> = routes.iter().map(|(tag, _)| tag.clone()).collect();
    let shared = tags[0].clone();
    let official = tags[1].clone();
    let tenant_a = tags[2].clone();
    let tenant_b = tags[3].clone();
    let retired = tags[4].clone();
    run_worker_for(
        first.clone(),
        vec![tenant_a.clone()],
        vec![tenant_a],
        settle,
    )
    .await;
    run_worker_for(
        next().await,
        vec![shared.clone()],
        vec![shared.clone()],
        settle,
    )
    .await;
    run_worker_for(
        next().await,
        vec![official.clone()],
        vec![official.clone()],
        settle,
    )
    .await;
    // One process may serve several targets, and then takes exactly those.
    run_worker_for(
        next().await,
        vec![tenant_b.clone(), official],
        vec![tenant_b],
        settle,
    )
    .await;
    // The retired class tag matches no target, so nothing is left for a target worker to take.
    run_worker_for(
        next().await,
        vec![shared, tags[1].clone(), tags[2].clone(), tags[3].clone()],
        vec![],
        settle,
    )
    .await;
    let _ = retired;
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
    each_worker_takes_only_its_own_target(first, Box::new(make), false, None, None).await;
}

#[tokio::test]
#[serial(queue_backend)]
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
        None,
        None,
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
#[serial(queue_backend)]
#[serial(process_environment)]
async fn redis_queue_delivers_each_target_only_to_its_worker() {
    redis_scenario(false).await;
}

#[tokio::test]
#[serial(queue_backend)]
#[serial(process_environment)]
async fn redis_queue_with_one_named_queue_per_target_delivers_only_to_its_worker() {
    redis_scenario(true).await;
}

async fn redis_scenario(named_queues: bool) {
    let Some(uri) = crate::valkey_test_url() else {
        return;
    };
    let run_id = Uuid::now_v7();
    let canonical_tags = [SHARED, OFFICIAL, TENANT_A, TENANT_B, RETIRED];
    let tags: Vec<String> = canonical_tags
        .iter()
        .map(|tag| format!("{tag}:test:{run_id}"))
        .collect();
    let all_queue = format!("queue-test:target-routing:{run_id}");
    let route_queues: Vec<String> = if named_queues {
        (0..canonical_tags.len())
            .map(|index| format!("{all_queue}:{index}"))
            .collect()
    } else {
        vec![all_queue; canonical_tags.len()]
    };
    let routes: Vec<(String, Option<String>)> = tags
        .iter()
        .cloned()
        .zip(route_queues.iter().cloned().map(Some))
        .collect();
    let queues: Vec<String> = route_queues
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let cleanup_uri = uri.clone();
    let make = move || {
        let uri = uri.clone();
        let queues = queues.clone();
        Box::pin(async move {
            let config = RedisQueueConfig {
                uri,
                dangerously_flush: false,
                queues: Some(queues),
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
    // Redis clear is FLUSHDB; never use it on the shared DB15 service.
    let owned_ids = Arc::new(Mutex::new(Vec::new()));
    let outcome = std::panic::AssertUnwindSafe(each_worker_takes_only_its_own_target(
        first.clone(),
        Box::new(make),
        named_queues,
        Some(routes),
        Some(owned_ids.clone()),
    ))
    .catch_unwind()
    .await;
    let _ = first.shutdown();
    let client =
        loco_rs::bgworker::redis::RedisPool::open(cleanup_uri).expect("open cleanup client");
    let mut connection = client
        .get_multiplexed_async_connection()
        .await
        .expect("connect cleanup client");
    let owned_ids = owned_ids.lock().expect("owned Redis jobs").clone();
    for (queue_name, id) in &owned_ids {
        let _: usize = redis::cmd("ZREM")
            .arg(format!("queue:{queue_name}"))
            .arg(id)
            .query_async(&mut connection)
            .await
            .expect("remove owned queued member");
        let _: usize = redis::cmd("SREM")
            .arg(format!("processing:{queue_name}"))
            .arg(id)
            .query_async(&mut connection)
            .await
            .expect("remove owned processing member");
        let _: usize = redis::cmd("DEL")
            .arg(format!("job:{id}"))
            .query_async(&mut connection)
            .await
            .expect("remove owned job key");
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
