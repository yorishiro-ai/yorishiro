//! Covers queue enqueue, priority ordering, worker tags, and provider boundaries.
//!
//! The isolated SQLite-provider cases use separate temporary application and queue files, preserving the permitted SQLite/SQLite pair.
//!
//! `tests/workers/queue_routing.rs` covers tag filtering through the public
//! `Queue::run(tags)` API for both SQL providers.  The Valkey provider is covered
//! by `uv run scripts/test_topology.py sqlite-valkey` against a real Valkey service.

use futures::FutureExt;
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use loco_rs::app::Hooks;
use loco_rs::bgworker::{BackgroundWorker, sqlt};
use loco_rs::boot::{self, BootResult};
use loco_rs::config::QueueConfig;
use loco_rs::environment::Environment;
use loco_rs::prelude::*;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::_entities::{queue_job_lifecycles, workspace_workspaces};
use yorishiro::workers::embedding_sync::{self, EmbeddingSyncArgs, WorkerClass};
use yorishiro::workers::reindex::{self, ReindexArgs};

use crate::requests::close_app_pools_sqlite;

/// Boots a permitted SQLite/SQLite pair with separate temporary files and returns a pool onto the queue file.
async fn with_sqlite_queue<F, Fut>(test: F)
where
    F: FnOnce(AppContext, sea_orm::sqlx::SqlitePool, Uuid) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let dir = tempfile::tempdir().expect("tempdir");
    let queue_path = dir.path().join("queue.sqlite3");
    let queue_uri = format!("sqlite://{}?mode=rwc", queue_path.display());

    // Use Option so we can explicitly clean up even on panic.
    let mut boot: Option<BootResult> = None;
    let mut queue_pool: Option<sea_orm::sqlx::SqlitePool> = None;

    let result = std::panic::AssertUnwindSafe(async {
        let database_uri = format!(
            "sqlite://{}?mode=rwc",
            dir.path().join("app.sqlite3").display()
        );
        let mut config = crate::sqlite_fixture_config(&database_uri);
        let Some(QueueConfig::Sqlite(queue)) = config.queue.as_mut() else {
            panic!("SQLite fixture must configure a SQLite queue");
        };
        queue.uri = queue_uri.clone();
        queue.dangerously_flush = false;

        let boot_res = App::boot(boot::StartMode::ServerOnly, &Environment::Test, config)
            .await
            .expect("boot with a sqlite queue");
        boot = Some(boot_res);
        let boot_res = boot.as_ref().expect("boot result");
        let pool = sea_orm::sqlx::SqlitePool::connect(&queue_uri)
            .await
            .expect("connect to the queue file");
        queue_pool = Some(pool.clone());

        let request = loco_rs::TestServer::new(
            boot_res
                .router
                .clone()
                .expect("booted router")
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .expect("build queue test server");
        let tenant = super::fixtures::TestTenant::create(
            &request,
            &boot_res.app_context,
            super::fixtures::TenantArgs::default(),
        )
        .await;
        test(
            boot_res.app_context.clone(),
            pool.clone(),
            tenant.workspace_id,
        )
        .await;
    })
    .catch_unwind()
    .await;

    // Close connections before the temporary database and queue directory is removed.
    if let Some(b) = &boot {
        App::on_shutdown(&b.app_context).await;
        if let Some(ref queue) = b.app_context.queue_provider {
            let _ = queue.shutdown();
        }
        close_app_pools_sqlite(
            &b.app_context,
            dir.path().join("app.sqlite3").to_str().unwrap(),
        )
        .await;
    }
    if let Some(pool) = queue_pool {
        pool.close().await;
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

async fn run_dispatch_relay(ctx: &AppContext) {
    yorishiro::workers::dispatch::recover_pending(ctx).await;
}

/// A job enqueued through `enqueue_for_class` reaches the configured queue.
#[tokio::test]
async fn enqueue_for_class_puts_a_row_in_the_queue() {
    if !crate::require_sqlite_backend() {
        return;
    }
    with_sqlite_queue(|ctx, pool, workspace_id| async move {
        embedding_sync::enqueue_for_class(&ctx, args_for(WorkerClass::Shared, workspace_id))
            .await
            .expect("enqueue");
        assert!(
            sqlt::get_jobs(&pool, None, None)
                .await
                .expect("queue remains untouched before the relay runs")
                .is_empty(),
            "the producer records durable intent but does not publish"
        );
        run_dispatch_relay(&ctx).await;

        let jobs = sqlt::get_jobs(&pool, None, None).await.expect("get_jobs");
        assert_eq!(jobs.len(), 1, "jobs: {jobs:?}");
        assert_eq!(jobs[0].name, "EmbeddingSyncWorker");
    })
    .await;
}

#[tokio::test]
async fn accepted_enqueue_with_failed_provider_id_record_keeps_the_outbox_row() {
    if !crate::require_sqlite_backend() {
        return;
    }
    with_sqlite_queue(|ctx, pool, workspace_id| async move {
        ctx.db
            .execute_unprepared(
                "CREATE TRIGGER fail_provider_job_id BEFORE UPDATE OF provider_job_id ON queue_job_lifecycles \
                 WHEN NEW.provider_job_id IS NOT NULL BEGIN \
                 SELECT RAISE(FAIL, 'test provider ID write failure'); END",
            )
            .await
            .expect("install temporary provider ID trigger");

        embedding_sync::enqueue_for_class(&ctx, args_for(WorkerClass::Shared, workspace_id))
            .await
            .expect("dispatch reports the provider acceptance");
        assert!(
            sqlt::get_jobs(&pool, None, None)
                .await
                .expect("queue remains untouched before the relay runs")
                .is_empty(),
            "the producer does not publish directly"
        );
        run_dispatch_relay(&ctx).await;

        let jobs = sqlt::get_jobs(&pool, None, None).await.expect("read queue");
        assert_eq!(jobs.len(), 1, "provider accepted one queue message");
        let lifecycle = queue_job_lifecycles::Entity::find()
            .one(&ctx.db)
            .await
            .expect("read lifecycle")
            .expect("lifecycle was recorded before enqueue");
        assert_eq!(lifecycle.status, "queued");
        assert_eq!(lifecycle.provider_job_id, None);
        let outbox = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(
            lifecycle.id,
        )
        .one(&ctx.db)
        .await
        .expect("read outbox")
        .expect("outbox remains after failed correlation write");
        assert!(
            yorishiro::models::queue_job_dispatch_outbox::due(&ctx.db)
                .await
                .expect("read cooldown-filtered outbox")
                .is_empty(),
            "the dispatch cooldown temporarily hides the accepted job"
        );
        let mut outbox = outbox.into_active_model();
        outbox.last_attempt_at = sea_orm::ActiveValue::Set(Some(
            Utc.timestamp_opt(1_000, 0).unwrap().fixed_offset(),
        ));
        outbox.update(&ctx.db).await.expect("age attempt timestamp");

        let due = yorishiro::models::queue_job_dispatch_outbox::due(&ctx.db)
            .await
            .expect("read recovered outbox");
        assert_eq!(due.len(), 1, "the accepted dispatch becomes eligible again");
        assert_eq!(due[0].lifecycle_id, Some(lifecycle.id));
        assert_eq!(due[0].payload, jobs[0].data);
    })
    .await;
}

/// Each `WorkerClass` enqueues under its own worker type and carries that class's tag.
///
/// `tags()` is a per-type static, so the class has to select the worker type at `perform_later` time rather than travel in the job's arguments; getting that backwards is the easy mistake here.
/// A regression that sent every class to one worker, or dropped the tag, shows up here as two rows sharing a name or carrying `None`.
#[tokio::test]
async fn each_worker_class_is_sent_to_its_own_target() {
    if !crate::require_sqlite_backend() {
        return;
    }
    with_sqlite_queue(|ctx, pool, workspace_id| async move {
        let tenant_id = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("read the workspace")
            .expect("the workspace exists")
            .tenant_id;
        for class in [
            WorkerClass::Shared,
            WorkerClass::Official,
            WorkerClass::TenantPrivate,
        ] {
            embedding_sync::enqueue_for_class(&ctx, args_for(class, workspace_id))
                .await
                .expect("enqueue");
        }
        assert!(
            sqlt::get_jobs(&pool, None, None)
                .await
                .expect("read queue before relay")
                .is_empty(),
            "producers do not publish worker jobs"
        );
        run_dispatch_relay(&ctx).await;

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
                    "EmbeddingSyncWorker".to_string(),
                    vec!["worker-target:official".to_string()],
                    200,
                ),
                (
                    "EmbeddingSyncWorker".to_string(),
                    vec!["worker-target:shared".to_string()],
                    100,
                ),
                (
                    "EmbeddingSyncWorker".to_string(),
                    vec![format!("worker-target:tenant:{tenant_id}")],
                    300,
                ),
            ]
        );
    })
    .await;
}

#[tokio::test]
async fn each_reindex_worker_class_is_sent_to_its_own_target() {
    if !crate::require_sqlite_backend() {
        return;
    }
    with_sqlite_queue(|ctx, pool, workspace_id| async move {
        let tenant_id = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("read the workspace")
            .expect("the workspace exists")
            .tenant_id;
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
                    startup: false,
                },
            )
            .await
            .expect("enqueue");
        }
        assert!(
            sqlt::get_jobs(&pool, None, None)
                .await
                .expect("read queue before relay")
                .is_empty(),
            "reindex producers do not publish worker jobs"
        );
        run_dispatch_relay(&ctx).await;

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
                    "ReindexWorker".to_string(),
                    vec!["worker-target:official".to_string()],
                    200,
                ),
                (
                    "ReindexWorker".to_string(),
                    vec!["worker-target:shared".to_string()],
                    100,
                ),
                (
                    "ReindexWorker".to_string(),
                    vec![format!("worker-target:tenant:{tenant_id}")],
                    300,
                ),
            ]
        );
    })
    .await;
}

#[tokio::test]
async fn configured_queue_orders_competing_jobs_by_priority() {
    use loco_rs::bgworker::JobStatus;
    use yorishiro::models::query_embedding_requests::Entity as Requests;
    use yorishiro::services::embedding::{
        EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
    };
    use yorishiro::workers::query_embedding::{QueryEmbeddingArgs, QueryEmbeddingWorker};
    use yorishiro::workers::target::WorkerTarget;

    let (arrived, mut arrivals) = tokio::sync::mpsc::unbounded_channel();
    let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let arrived = arrived.clone();
            async move {
                arrived
                    .send(body["input"][0].as_str().unwrap().to_owned())
                    .unwrap();
                axum::Json(
                    serde_json::json!({"data": [{"index": 0, "embedding": vec![1.0_f32; 768]}]}),
                )
            }
        }),
    ))
    .await;
    super::boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = super::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let provider = Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: server.url.clone(),
            api_key: "priority-test".into(),
            model: "priority-test".into(),
            dimensions: 768,
            send_dimensions_param: false,
        })) as Arc<dyn EmbeddingProvider>;
        ctx.shared_store.insert(provider);
        let mut worker_ctx = ctx.clone();
        // Ordering is observable with one consumer, as with a single-worker deployment.
        match worker_ctx.config.queue.as_mut().unwrap() {
            QueueConfig::Sqlite(config) => config.num_workers = 1,
            QueueConfig::Postgres(config) => config.num_workers = 1,
            QueueConfig::Redis(config) => config.num_workers = 1,
            _ => panic!("unsupported queue"),
        }
        let queue = super::query_worker::shared_target_queue(&worker_ctx).await;
        worker_ctx.queue_provider = Some(queue.clone());
        App::connect_workers(&worker_ctx, &queue).await.unwrap();
        let mut ids = Vec::new();
        // Fault-inject competing provider deliveries with explicit priorities; request creation and execution use production models and workers.
        for (text, priority) in [("low", 100), ("high", 300)] {
            let request_id = Requests::open(
                &ctx.db,
                tenant.workspace_id,
                text,
                60,
                chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
            )
            .await
            .unwrap();
            ids.push(
                queue
                    .enqueue(
                        QueryEmbeddingWorker::class_name(),
                        Some(WorkerTarget::Shared.queue()),
                        QueryEmbeddingArgs {
                            request_id,
                            workspace_id: tenant.workspace_id,
                        },
                        Some(vec![WorkerTarget::Shared.tag()]),
                        Some(priority),
                    )
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        let running = queue.clone();
        let worker =
            tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
        let outcome =
            std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(5), async {
                assert_eq!(arrivals.recv().await.as_deref(), Some("high"));
                assert_eq!(arrivals.recv().await.as_deref(), Some("low"));
                let directory = tempfile::tempdir().unwrap();
                loop {
                    let dump = queue.dump(directory.path(), None, None).await.unwrap();
                    let jobs: Vec<loco_rs::bgworker::Job> =
                        serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                    if ids.iter().all(|id| {
                        jobs.iter()
                            .any(|job| job.id == *id && job.status == JobStatus::Completed)
                    }) {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            }))
            .catch_unwind()
            .await;
        App::on_shutdown(&worker_ctx).await;
        queue.shutdown().unwrap();
        let drained = tokio::time::timeout(Duration::from_secs(10), worker).await;
        queue.close().await.unwrap();
        match outcome {
            Ok(result) => result.expect("priority deliveries complete"),
            Err(panic) => std::panic::resume_unwind(panic),
        }
        drained.unwrap().unwrap().unwrap();
    })
    .await;
}
