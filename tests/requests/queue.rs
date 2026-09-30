//! Covers the enqueue side of the background queue: that `enqueue_for_class` puts a row in the queue at all, and that each `WorkerClass` carries its own tag.
//!
//! Nothing else in this suite exercises a queue backend.
//! `config/test.yaml` sets `workers.mode: ForegroundBlocking`, under which `perform_later` calls `perform` inline and never touches a queue, so every job body is covered on every run and the enqueue path would otherwise be covered by nothing: a change breaking it ships through a green suite.
//!
//! `tests/workers/queue_routing.rs` covers the dequeue-side tag filter through `Queue::run(tags)`.
//! It uses a probe worker and both SQL providers rather than copying Loco's filter SQL into this crate.
//!
//! The application database is PostgreSQL (`request_with_create_db`) while the queue is a SQLite file in a `TempDir`, which is a pairing no deployment runs.
//! It is inert with respect to what is asserted rather than merely tolerated: the queue provider opens its own `sqlx::SqlitePool` against its own URI (`bgworker/mod.rs`) and has no view of `ctx.db` at all, which is the same independence that lets the database and queue share one file in `config/development.yaml`.
//! Booting the whole application on SQLite instead would be more faithful and would bring the entire `tests/`-is-PostgreSQL-only question with it, which is a much larger surface than two assertions justify.

use futures::FutureExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use loco_rs::app::Hooks;
use loco_rs::bgworker::{self, BackgroundWorker, Queue, sqlt};
use loco_rs::boot::{self, BootResult};
use loco_rs::config::{
    PostgresQueueConfig, QueueConfig, RedisQueueConfig, SqliteQueueConfig, WorkerMode,
};
use loco_rs::environment::Environment;
use loco_rs::prelude::*;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::models::_entities::{tenant_billing, tenant_tenants, workspace_workspaces};
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::workers::embedding_sync::{self, EmbeddingSyncArgs, WorkerClass};
use yorishiro::workers::reindex::{self, ReindexArgs};

use crate::requests::close_app_pools;

const COMPETING_TAG: &str = "queue-test:competing";
static COMPETING_ORDER: std::sync::OnceLock<Mutex<Vec<(String, String)>>> =
    std::sync::OnceLock::new();
static COMPETING_PROGRESS: std::sync::OnceLock<tokio::sync::watch::Sender<HashMap<String, usize>>> =
    std::sync::OnceLock::new();

#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct CompetingArgs {
    run_id: String,
    name: String,
}

struct CompetingWorker;

#[async_trait]
impl BackgroundWorker<CompetingArgs> for CompetingWorker {
    fn build(_ctx: &AppContext) -> Self {
        Self
    }

    fn tags() -> Vec<String> {
        vec![COMPETING_TAG.to_owned()]
    }

    async fn perform(&self, args: CompetingArgs) -> loco_rs::Result<()> {
        COMPETING_ORDER
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .expect("competing order lock")
            .push((args.run_id.clone(), args.name));
        if let Some(progress) = COMPETING_PROGRESS.get() {
            let mut counts = progress.borrow().clone();
            *counts.entry(args.run_id).or_default() += 1;
            let _ = progress.send(counts);
        }
        Ok(())
    }
}

async fn assert_competing_priority_order(queue: Arc<Queue>) {
    queue
        .register(CompetingWorker)
        .await
        .expect("register competing worker");
    queue
        .enqueue(
            CompetingWorker::class_name(),
            None,
            CompetingArgs {
                run_id: "priority".to_owned(),
                name: "low".to_owned(),
            },
            Some(vec![COMPETING_TAG.to_owned()]),
            Some(100),
        )
        .await
        .expect("enqueue low priority job");
    queue
        .enqueue(
            CompetingWorker::class_name(),
            None,
            CompetingArgs {
                run_id: "priority".to_owned(),
                name: "high".to_owned(),
            },
            Some(vec![COMPETING_TAG.to_owned()]),
            Some(300),
        )
        .await
        .expect("enqueue high priority job");

    finish_competing_order(queue, "priority", vec!["high", "low"]).await;
}

async fn finish_competing_order(queue: Arc<Queue>, run_id: &str, expected: Vec<&str>) {
    let progress = COMPETING_PROGRESS.get_or_init(|| {
        let (sender, _receiver) = tokio::sync::watch::channel(HashMap::new());
        sender
    });
    let mut counts = progress.borrow().clone();
    counts.insert(run_id.to_owned(), 0);
    let _ = progress.send(counts);
    let mut progress = progress.subscribe();
    let running_queue = queue.clone();
    let handle =
        tokio::spawn(async move { running_queue.run(vec![COMPETING_TAG.to_owned()]).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while progress.borrow().get(run_id).copied().unwrap_or_default() < 2 {
            progress.changed().await.expect("competing worker progress");
        }
    })
    .await
    .expect("competing jobs completed");
    queue.shutdown().expect("stop competing worker");
    handle
        .await
        .expect("join competing worker")
        .expect("run competing worker");
    let actual = COMPETING_ORDER
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .expect("competing order lock")
        .iter()
        .filter(|(existing_run, _)| existing_run == run_id)
        .map(|(_, name)| name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        expected.into_iter().map(str::to_owned).collect::<Vec<_>>()
    );
}

/// Boots the app against a throwaway PostgreSQL database with `BackgroundQueue` and a SQLite queue file, and hands the test both the context and a pool onto that same queue file.
///
/// `config/test.yaml` carries no `queue:` block, so `BackgroundQueue` there fails outright with `QueueProviderMissing`.
/// The config is therefore supplied here rather than by flipping a mode: `H::load_config` returns an owned `Config`, and `boot_test_with_create_db` already mutates `database.uri` on it before calling `H::boot`, so setting `workers` and `queue` the same way touches nothing process-wide and leaves every `ForegroundBlocking` test in this binary unaffected.
async fn with_sqlite_queue<F, Fut>(test: F)
where
    F: FnOnce(AppContext, sqlx::SqlitePool, Uuid) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let dir = tempfile::tempdir().expect("tempdir");
    let queue_path = dir.path().join("queue.sqlite3");
    let queue_uri = format!("sqlite://{}?mode=rwc", queue_path.display());

    // Use Option so we can explicitly clean up even on panic.
    let mut test_db: Option<Box<dyn loco_rs::testing::db::TestSupport>> = None;
    let mut boot: Option<BootResult> = None;

    let result = std::panic::AssertUnwindSafe(async {
        let mut config = App::load_config(&Environment::Test)
            .await
            .expect("load test config");
        // The queue provider's workers need PG connections during boot, so the
        // pool must be large enough.  Bump the connect timeout so the queue
        // workers don't block the migration phase.
        config.database.connect_timeout = 30_000;
        let db = loco_rs::testing::db::init_test_db_creation(&config.database.uri)
            .expect("init test db");
        db.init_db().await;
        config.database.uri = db.get_connection_str().to_string();
        config.workers.mode = WorkerMode::BackgroundQueue;
        config.queue = Some(QueueConfig::Sqlite(SqliteQueueConfig {
            uri: queue_uri.clone(),
            dangerously_flush: false,
            enable_logging: false,
            max_connections: 2,
            min_connections: 1,
            connect_timeout: 5000,
            idle_timeout: 5000,
            poll_interval_sec: 1,
            num_workers: 1,
            reaper: None,
        }));

        let boot_res = App::boot(boot::StartMode::ServerOnly, &Environment::Test, config)
            .await
            .expect("boot with a sqlite queue");

        let pool = sqlx::SqlitePool::connect(&queue_uri)
            .await
            .expect("connect to the queue file");

        let tenant = tenant_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("queue-test-tenant".into()),
            ..Default::default()
        }
        .insert(&boot_res.app_context.db)
        .await
        .expect("insert queue test tenant");
        let workspace = workspace_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("queue-test-workspace".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_owned()),
            ..Default::default()
        }
        .insert(&boot_res.app_context.db)
        .await
        .expect("insert queue test workspace");
        tenant_billing::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            plan: sea_orm::ActiveValue::Set(Some("free".into())),
            ..Default::default()
        }
        .insert(&boot_res.app_context.db)
        .await
        .expect("insert queue test billing");

        // Run the test with a real workspace and billing policy.
        test(boot_res.app_context.clone(), pool.clone(), workspace.id).await;

        // Shut down the queue provider first so its worker threads release their
        // PostgreSQL connections, then close pools — all inside the catch_unwind
        // block so this runs even when the test panics.
        if let Some(ref qp) = boot_res.app_context.queue_provider {
            let _ = qp.shutdown();
        }
        // Use close_app_pools which closes identity, tenant, and ctx.db pools
        // so DROP DATABASE does not fail on teardown.
        close_app_pools(&boot_res.app_context).await;
        pool.close().await;

        // Store for post-panic cleanup.
        test_db = Some(db);
        boot = Some(boot_res);
    })
    .catch_unwind()
    .await;

    // Post-panic cleanup: close app pools and drop the test DB.
    if let Some(b) = &boot {
        // close_app_pools closes identity, tenant, and ctx.db pools so
        // DROP DATABASE does not fail on teardown.
        close_app_pools(&b.app_context).await;
    }
    if let Some(d) = test_db.take() {
        d.cleanup_db();
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

fn args_for(class: WorkerClass, workspace_id: Uuid) -> EmbeddingSyncArgs {
    EmbeddingSyncArgs {
        lifecycle_id: None,
        workspace_id,
        entity_id: Uuid::now_v7(),
        worker_class: class,
    }
}

/// A job enqueued through `enqueue_for_class` reaches the queue at all.
///
/// In `ForegroundBlocking` this assertion is vacuous, since `perform_later` runs the body inline and writes no row, which is why nothing else here catches an enqueue-side break.
#[tokio::test]
async fn enqueue_for_class_puts_a_row_in_the_queue() {
    if !super::super::require_postgres_backend() {
        return;
    }
    with_sqlite_queue(|ctx, pool, workspace_id| async move {
        embedding_sync::enqueue_for_class(&ctx, args_for(WorkerClass::Shared, workspace_id))
            .await
            .expect("enqueue");

        let jobs = sqlt::get_jobs(&pool, None, None).await.expect("get_jobs");
        assert_eq!(jobs.len(), 1, "jobs: {jobs:?}");
        assert_eq!(jobs[0].name, "EmbeddingSyncWorkerShared");
    })
    .await;
}

/// Each `WorkerClass` enqueues under its own worker type and carries that class's tag.
///
/// `tags()` is a per-type static, so the class has to select the worker type at `perform_later` time rather than travel in the job's arguments; getting that backwards is the easy mistake here.
/// A regression that sent every class to one worker, or dropped the tag, shows up here as two rows sharing a name or carrying `None`.
#[tokio::test]
async fn each_worker_class_carries_its_own_tag() {
    if !super::super::require_postgres_backend() {
        return;
    }
    with_sqlite_queue(|ctx, pool, workspace_id| async move {
        for class in [
            WorkerClass::Shared,
            WorkerClass::Official,
            WorkerClass::TenantPrivate,
        ] {
            embedding_sync::enqueue_for_class(&ctx, args_for(class, workspace_id))
                .await
                .expect("enqueue");
        }

        let jobs = sqlt::get_jobs(&pool, None, None).await.expect("get_jobs");
        assert_eq!(jobs.len(), 3, "jobs: {jobs:?}");

        let mut seen: Vec<(String, Vec<String>, i32)> = jobs
            .iter()
            .map(|job| {
                (
                    job.name.clone(),
                    job.tags.clone().unwrap_or_default(),
                    job.priority,
                )
            })
            .collect();
        seen.sort();

        assert_eq!(
            seen,
            vec![
                (
                    "EmbeddingSyncWorkerOfficial".to_string(),
                    vec!["worker-class:official".to_string()],
                    200,
                ),
                (
                    "EmbeddingSyncWorkerShared".to_string(),
                    vec!["worker-class:shared".to_string()],
                    100,
                ),
                (
                    "EmbeddingSyncWorkerTenantPrivate".to_string(),
                    vec!["worker-class:tenant-private".to_string()],
                    300,
                ),
            ]
        );
    })
    .await;
}

#[tokio::test]
async fn each_reindex_worker_class_carries_its_own_tag() {
    if !super::super::require_postgres_backend() {
        return;
    }
    with_sqlite_queue(|ctx, pool, workspace_id| async move {
        for class in [
            WorkerClass::Shared,
            WorkerClass::Official,
            WorkerClass::TenantPrivate,
        ] {
            reindex::enqueue_for_class(
                &ctx,
                ReindexArgs {
                    lifecycle_id: None,
                    workspace_id,
                    worker_class: class,
                },
            )
            .await
            .expect("enqueue");
        }

        let jobs = sqlt::get_jobs(&pool, None, None).await.expect("get_jobs");
        assert_eq!(jobs.len(), 3, "jobs: {jobs:?}");

        let mut seen: Vec<(String, Vec<String>, i32)> = jobs
            .iter()
            .map(|job| {
                (
                    job.name.clone(),
                    job.tags.clone().unwrap_or_default(),
                    job.priority,
                )
            })
            .collect();
        seen.sort();

        assert_eq!(
            seen,
            vec![
                (
                    "ReindexWorkerOfficial".to_string(),
                    vec!["worker-class:official".to_string()],
                    200,
                ),
                (
                    "ReindexWorkerShared".to_string(),
                    vec!["worker-class:shared".to_string()],
                    100,
                ),
                (
                    "ReindexWorkerTenantPrivate".to_string(),
                    vec!["worker-class:tenant-private".to_string()],
                    300,
                ),
            ]
        );
    })
    .await;
}

#[tokio::test]
#[serial_test::serial(queue_postgres)]
async fn sqlite_background_queue_orders_competing_jobs_without_sleeping() {
    let directory = tempfile::tempdir().expect("queue tempdir");
    let uri = format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("competing.sqlite3").display()
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
    let provider = bgworker::sqlt::create_provider(&config)
        .await
        .expect("SQLite queue");
    let queue = Arc::new(provider);
    queue.setup().await.expect("set up SQLite queue");
    assert_competing_priority_order(queue).await;
}

#[tokio::test]
#[serial_test::serial(queue_postgres)]
async fn postgres_background_queue_orders_competing_jobs_without_sleeping() {
    if !super::super::require_postgres_backend() {
        return;
    }
    let config = PostgresQueueConfig {
        uri: std::env::var("DATABASE_URL").expect("PostgreSQL DATABASE_URL"),
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
    let provider = bgworker::pg::create_provider(&config)
        .await
        .expect("Postgres queue");
    let queue = Arc::new(provider);
    queue.setup().await.expect("set up Postgres queue");
    assert_competing_priority_order(queue).await;
}

#[tokio::test]
#[serial_test::serial(queue_postgres)]
async fn sqlite_equal_priority_order_uses_controlled_job_ids() {
    let run_id = "equal-sqlite";
    let directory = tempfile::tempdir().expect("queue tempdir");
    let uri = format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("equal.sqlite3").display()
    );
    let config = SqliteQueueConfig {
        uri: uri.clone(),
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
    let provider = bgworker::sqlt::create_provider(&config)
        .await
        .expect("SQLite queue");
    let queue = Arc::new(provider);
    queue.setup().await.expect("set up SQLite queue");
    queue
        .register(CompetingWorker)
        .await
        .expect("register competing worker");
    let pool = sqlx::SqlitePool::connect(&uri)
        .await
        .expect("connect SQLite queue");
    for (id, name) in [("job-b", "id-b"), ("job-a", "id-a")] {
        let generated_id = queue
            .enqueue(
                CompetingWorker::class_name(),
                None,
                CompetingArgs {
                    run_id: run_id.to_owned(),
                    name: name.to_owned(),
                },
                Some(vec![COMPETING_TAG.to_owned()]),
                Some(200),
            )
            .await
            .expect("enqueue SQLite equal-priority job");
        sqlx::query("UPDATE sqlt_loco_queue SET id = ?, run_at = ? WHERE id = ?")
            .bind(id)
            .bind("2000-01-01 00:00:00")
            .bind(generated_id)
            .execute(&pool)
            .await
            .expect("control SQLite equal-priority job");
    }
    let ordered: Vec<(String,)> = sqlx::query_as(
        "SELECT id FROM sqlt_loco_queue WHERE status = 'queued' ORDER BY priority DESC, run_at, id",
    )
    .fetch_all(&pool)
    .await
    .expect("read SQLite equal-priority order");
    assert_eq!(ordered, vec![("job-a".to_owned(),), ("job-b".to_owned(),)]);
    pool.close().await;
}

#[tokio::test]
#[serial_test::serial(queue_postgres)]
async fn postgres_equal_priority_order_uses_controlled_job_ids() {
    if !super::super::require_postgres_backend() {
        return;
    }
    let uri = std::env::var("DATABASE_URL").expect("PostgreSQL DATABASE_URL");
    let run_id = "equal-postgres";
    let config = PostgresQueueConfig {
        uri: uri.clone(),
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
    let provider = bgworker::pg::create_provider(&config)
        .await
        .expect("Postgres queue");
    let queue = Arc::new(provider);
    queue.setup().await.expect("set up Postgres queue");
    let pool = sqlx::PgPool::connect(&uri)
        .await
        .expect("connect Postgres queue");
    for (id, name) in [("job-b", "id-b"), ("job-a", "id-a")] {
        let generated_id = queue
            .enqueue(
                CompetingWorker::class_name(),
                None,
                CompetingArgs {
                    run_id: run_id.to_owned(),
                    name: name.to_owned(),
                },
                Some(vec![COMPETING_TAG.to_owned()]),
                Some(200),
            )
            .await
            .expect("enqueue Postgres equal-priority job");
        sqlx::query("UPDATE pg_loco_queue SET id = $1, run_at = $2 WHERE id = $3")
            .bind(id)
            .bind("2000-01-01T00:00:00Z")
            .bind(generated_id)
            .execute(&pool)
            .await
            .expect("control Postgres equal-priority job");
    }
    let ordered: Vec<(String,)> = sqlx::query_as(
        "SELECT id FROM pg_loco_queue WHERE status = 'queued' ORDER BY priority DESC, run_at, id",
    )
    .fetch_all(&pool)
    .await
    .expect("read Postgres equal-priority order");
    assert_eq!(ordered, vec![("job-a".to_owned(),), ("job-b".to_owned(),)]);
    pool.close().await;
}

#[tokio::test]
#[serial_test::serial(queue_postgres)]
async fn redis_bounded_scan_is_observable_at_the_queue_boundary() {
    let Ok(uri) = std::env::var("QUEUE_URL") else {
        return;
    };
    if !uri.starts_with("redis://") && !uri.starts_with("rediss://") {
        return;
    }
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
    queue.setup().await.expect("set up Redis queue");
    queue
        .register(CompetingWorker)
        .await
        .expect("register competing worker");
    let run_id = "bounded-redis";
    for index in 0..1001 {
        queue
            .enqueue(
                CompetingWorker::class_name(),
                None,
                CompetingArgs {
                    run_id: run_id.to_owned(),
                    name: format!("mismatch-{index}"),
                },
                Some(vec!["queue-test:other".to_owned()]),
                Some(200),
            )
            .await
            .expect("enqueue Redis mismatch");
    }
    queue
        .enqueue(
            CompetingWorker::class_name(),
            None,
            CompetingArgs {
                run_id: run_id.to_owned(),
                name: "matching-beyond-scan".to_owned(),
            },
            Some(vec![COMPETING_TAG.to_owned()]),
            Some(200),
        )
        .await
        .expect("enqueue Redis matching job");
    let progress = COMPETING_PROGRESS.get_or_init(|| {
        let (sender, _receiver) = tokio::sync::watch::channel(HashMap::new());
        sender
    });
    let mut counts = progress.borrow().clone();
    counts.insert(run_id.to_owned(), 0);
    let _ = progress.send(counts);
    let mut progress = progress.subscribe();
    let running_queue = queue.clone();
    let handle =
        tokio::spawn(async move { running_queue.run(vec![COMPETING_TAG.to_owned()]).await });
    assert!(
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if progress.borrow().get(run_id).copied().unwrap_or_default() > 0 {
                    return;
                }
                progress.changed().await.expect("Redis worker progress");
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_err(),
        "matching job outside Redis bounded scan was processed"
    );
    queue.shutdown().expect("stop Redis worker");
    handle
        .await
        .expect("join Redis worker")
        .expect("run Redis worker");
    queue.clear().await.expect("clear Redis queue");
}
