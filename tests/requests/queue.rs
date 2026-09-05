//! Covers the enqueue side of the background queue: that `enqueue_for_class` puts a row in the queue at all, and that each `WorkerClass` carries its own tag.
//!
//! Nothing else in this suite exercises a queue backend.
//! `config/test.yaml` sets `workers.mode: ForegroundBlocking`, under which `perform_later` calls `perform` inline and never touches a queue, so every job body is covered on every run and the enqueue path would otherwise be covered by nothing: a change breaking it ships through a green suite.
//!
//! **What this does not cover: the dequeue-side tag filter.**
//! A change can break both the enqueue join tested here and the `WHERE` clause that decides which tags a running worker takes, and only the first is reachable from a test.
//! `Queue::run(tags)` is the sole public entry to the dequeue side and it is a loop with no one-shot, so asserting on it means running a worker for a bounded interval and checking what happened, which is a timing test.
//! `SqliteDriver::dequeue` is the primitive that would express it exactly, and it is unreachable: the `Driver` trait lives in `bgworker::sql`, which is `pub(crate)`.
//! **Do not close this gap by re-running loco's own filter SQL against the queue table.** A copy of an implementation passes whether or not the implementation still behaves that way: it would keep passing after a loco upgrade changed the real filter, while appearing to cover exactly what changed.
//! If loco ever makes `Driver` public, that assertion becomes writable and belongs here.
//!
//! The application database is PostgreSQL (`request_with_create_db`) while the queue is a SQLite file in a `TempDir`, which is a pairing no deployment runs.
//! It is inert with respect to what is asserted rather than merely tolerated: the queue provider opens its own `sqlx::SqlitePool` against its own URI (`bgworker/mod.rs`) and has no view of `ctx.db` at all, which is the same independence that lets the database and queue share one file in `config/development.yaml`.
//! Booting the whole application on SQLite instead would be more faithful and would bring the entire `tests/`-is-PostgreSQL-only question with it, which is a much larger surface than two assertions justify.

use futures::FutureExt;
use loco_rs::app::Hooks;
use loco_rs::bgworker::sqlt;
use loco_rs::boot::{self, BootResult};
use loco_rs::config::{QueueConfig, SqliteQueueConfig, WorkerMode};
use loco_rs::environment::Environment;
use loco_rs::prelude::*;
use serial_test::serial;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::workers::embedding_sync::{self, EmbeddingSyncArgs, WorkerClass};

use crate::requests::close_app_pools;

/// Boots the app against a throwaway PostgreSQL database with `BackgroundQueue` and a SQLite queue file, and hands the test both the context and a pool onto that same queue file.
///
/// `config/test.yaml` carries no `queue:` block, so `BackgroundQueue` there fails outright with `QueueProviderMissing`.
/// The config is therefore supplied here rather than by flipping a mode: `H::load_config` returns an owned `Config`, and `boot_test_with_create_db` already mutates `database.uri` on it before calling `H::boot`, so setting `workers` and `queue` the same way touches nothing process-wide and leaves every `ForegroundBlocking` test in this binary unaffected.
async fn with_sqlite_queue<F, Fut>(test: F)
where
    F: FnOnce(AppContext, sqlx::SqlitePool) -> Fut,
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
            num_workers: 0,
            reaper: None,
        }));

        let boot_res = App::boot(boot::StartMode::ServerOnly, &Environment::Test, config)
            .await
            .expect("boot with a sqlite queue");

        let pool = sqlx::SqlitePool::connect(&queue_uri)
            .await
            .expect("connect to the queue file");

        // Run the test.
        test(boot_res.app_context.clone(), pool.clone()).await;

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

fn args_for(class: WorkerClass) -> EmbeddingSyncArgs {
    EmbeddingSyncArgs {
        workspace_id: Uuid::now_v7(),
        entity_id: Uuid::now_v7(),
        worker_class: class,
    }
}

/// A job enqueued through `enqueue_for_class` reaches the queue at all.
///
/// In `ForegroundBlocking` this assertion is vacuous, since `perform_later` runs the body inline and writes no row, which is why nothing else here catches an enqueue-side break.
#[tokio::test]
#[serial]
async fn enqueue_for_class_puts_a_row_in_the_queue() {
    if super::super::require_sqlite_backend() {
        return;
    }
    with_sqlite_queue(|ctx, pool| async move {
        embedding_sync::enqueue_for_class(&ctx, args_for(WorkerClass::Shared))
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
#[serial]
async fn each_worker_class_carries_its_own_tag() {
    if super::super::require_sqlite_backend() {
        return;
    }
    with_sqlite_queue(|ctx, pool| async move {
        for class in [
            WorkerClass::Shared,
            WorkerClass::Official,
            WorkerClass::TenantPrivate,
        ] {
            embedding_sync::enqueue_for_class(&ctx, args_for(class))
                .await
                .expect("enqueue");
        }

        let jobs = sqlt::get_jobs(&pool, None, None).await.expect("get_jobs");
        assert_eq!(jobs.len(), 3, "jobs: {jobs:?}");

        let mut seen: Vec<(String, Vec<String>)> = jobs
            .iter()
            .map(|job| (job.name.clone(), job.tags.clone().unwrap_or_default()))
            .collect();
        seen.sort();

        assert_eq!(
            seen,
            vec![
                (
                    "EmbeddingSyncWorkerOfficial".to_string(),
                    vec!["worker-class:official".to_string()]
                ),
                (
                    "EmbeddingSyncWorkerShared".to_string(),
                    vec!["worker-class:shared".to_string()]
                ),
                (
                    "EmbeddingSyncWorkerTenantPrivate".to_string(),
                    vec!["worker-class:tenant-private".to_string()]
                ),
            ]
        );
    })
    .await;
}
