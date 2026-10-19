use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::async_trait;
use loco_rs::prelude::*;
use migration::MigratorTrait;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

use yorishiro::edition::ee::workers::infer_fill::*;

#[derive(Default)]
struct StalledQueue {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait]
impl loco_rs::bgworker::QueueProvider for StalledQueue {
    async fn enqueue(
        &self,
        _class: String,
        _queue: Option<String>,
        _args: serde_json::Value,
        _tags: Option<Vec<String>>,
        _priority: Option<i32>,
    ) -> loco_rs::Result<Option<String>> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(Some("stalled-provider-job".into()))
    }

    async fn register_handler(
        &self,
        _name: String,
        _handler: loco_rs::bgworker::JobHandler,
    ) -> loco_rs::Result<()> {
        Ok(())
    }

    async fn run(&self, _tags: Vec<String>) -> loco_rs::Result<()> {
        Ok(())
    }

    async fn setup(&self) -> loco_rs::Result<()> {
        Ok(())
    }

    async fn clear(&self) -> loco_rs::Result<()> {
        Ok(())
    }

    async fn ping(&self) -> loco_rs::Result<()> {
        Ok(())
    }

    async fn get_jobs(
        &self,
        _status: Option<&Vec<loco_rs::bgworker::JobStatus>>,
        _age_days: Option<i64>,
    ) -> loco_rs::Result<Vec<loco_rs::bgworker::Job>> {
        Ok(Vec::new())
    }

    async fn cancel_jobs_by_name(&self, _name: &str) -> loco_rs::Result<()> {
        Ok(())
    }

    async fn clear_by_status(
        &self,
        _status: Vec<loco_rs::bgworker::JobStatus>,
    ) -> loco_rs::Result<()> {
        Ok(())
    }

    async fn clear_jobs_older_than(
        &self,
        _age_days: i64,
        _status: Option<&Vec<loco_rs::bgworker::JobStatus>>,
    ) -> loco_rs::Result<()> {
        Ok(())
    }

    async fn requeue(&self, _age_minutes: &i64) -> loco_rs::Result<()> {
        Ok(())
    }

    async fn retry_failed(&self, _id: Option<&str>) -> loco_rs::Result<u64> {
        Ok(0)
    }

    fn describe(&self) -> String {
        "stalled test queue".into()
    }

    fn shutdown(&self) -> loco_rs::Result<()> {
        Ok(())
    }
}

struct RecordingDispatcher {
    args: Mutex<Vec<InferFillArgs>>,
    fail: bool,
}

#[async_trait]
impl InferFillDispatcher for RecordingDispatcher {
    async fn dispatch(
        &self,
        _ctx: &AppContext,
        args: InferFillArgs,
        _txn: Option<&sea_orm::DatabaseTransaction>,
    ) -> loco_rs::Result<String> {
        self.args.lock().unwrap().push(args);
        if self.fail {
            Err(loco_rs::Error::Message("dispatch failed".into()))
        } else {
            Ok("infer-fill-job".into())
        }
    }
}

#[tokio::test]
async fn dispatches_the_original_args_and_returns_success() {
    let ctx = crate::workers::test_context().await;
    let args = InferFillArgs {
        lifecycle_id: None,
        job_id: Uuid::now_v7(),
        workspace_id: Uuid::now_v7(),
        schema_name: "note".into(),
    };
    let dispatcher = RecordingDispatcher {
        args: Mutex::new(Vec::new()),
        fail: false,
    };

    let job_id = enqueue_infer_fill_with_dispatcher(&ctx, args.clone(), &dispatcher)
        .await
        .expect("dispatch");

    assert_eq!(job_id, "infer-fill-job");
    let recorded = dispatcher.args.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].job_id, args.job_id);
    assert_eq!(recorded[0].workspace_id, args.workspace_id);
    assert_eq!(recorded[0].schema_name, args.schema_name);
}

#[tokio::test]
async fn preserves_dispatch_failure() {
    let ctx = crate::workers::test_context().await;
    let dispatcher = RecordingDispatcher {
        args: Mutex::new(Vec::new()),
        fail: true,
    };

    let error = enqueue_infer_fill_with_dispatcher(
        &ctx,
        InferFillArgs {
            lifecycle_id: None,
            job_id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
            schema_name: "note".into(),
        },
        &dispatcher,
    )
    .await
    .expect_err("dispatch must fail");

    assert_eq!(error.to_string(), "dispatch failed");
}

#[tokio::test]
async fn rejects_a_missing_queue_before_creating_a_job() {
    let ctx = crate::workers::test_context().await;

    let error = enqueue_infer_fill(&ctx, Uuid::now_v7(), "note".into())
        .await
        .expect_err("missing queue must fail");

    assert_eq!(
        error.to_string(),
        "infer-fill requires a queue provider (configure queue: in the server config)"
    );
}

/// Infer-fill jobs travel to the shared target when they are enqueued, so the worker declares no tag or queue of its own.
/// Loco stores the class name on every queued job and finds the handler by it, so renaming the worker strands whatever is already queued.
#[test]
fn infer_fill_worker_declares_no_routing_and_keeps_its_class_name() {
    assert!(InferFillWorker::tags().is_empty());
    assert!(InferFillWorker::queue().is_none());
    assert_eq!(InferFillWorker::class_name(), "InferFillWorker");
}

#[tokio::test]
async fn relay_dispatches_infer_fill_using_the_registered_worker_class() {
    use loco_rs::bgworker::{self, BackgroundWorker};
    use loco_rs::config::SqliteQueueConfig;
    use sea_orm::{ActiveModelTrait, Set};
    use yorishiro::models::queue_job_lifecycles::Enqueue;
    use yorishiro::workers::target::WorkerTarget;

    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("app.sqlite3");
    let db = sea_orm::Database::connect(format!("sqlite://{}?mode=rwc", database_path.display()))
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();
    let mut config =
        crate::sqlite_fixture_config(&format!("sqlite://{}?mode=rwc", database_path.display()));
    if let Some(loco_rs::config::QueueConfig::Sqlite(queue)) = config.queue.as_mut() {
        queue.uri = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("queue.sqlite3").display()
        );
    }
    let ctx = AppContext::builder(
        loco_rs::environment::Environment::Test,
        db.clone(),
        config.clone(),
    )
    .build();
    ctx.shared_store.insert(
        config
            .settings::<yorishiro::data::settings::Settings>()
            .unwrap(),
    );
    let queue_uri = match config.queue.as_ref().unwrap() {
        loco_rs::config::QueueConfig::Sqlite(queue) => queue.uri.clone(),
        _ => unreachable!(),
    };
    let queue = Arc::new(
        bgworker::sqlt::create_provider(&SqliteQueueConfig {
            uri: queue_uri.clone(),
            dangerously_flush: false,
            enable_logging: false,
            max_connections: 2,
            min_connections: 1,
            connect_timeout: 5_000,
            idle_timeout: 5_000,
            poll_interval_sec: 0,
            num_workers: 1,
            reaper: None,
        })
        .await
        .unwrap(),
    );
    queue.setup().await.unwrap();
    queue.register(InferFillWorker::build(&ctx)).await.unwrap();
    let ctx = ctx.into_builder().queue_provider(queue.clone()).build();

    let tenant_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
    yorishiro::models::_entities::tenant_tenants::ActiveModel {
        id: Set(tenant_id),
        name: Set(format!("tenant-{tenant_id}")),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    yorishiro::models::_entities::workspace_workspaces::ActiveModel {
        id: Set(workspace_id),
        tenant_id: Set(tenant_id),
        name: Set(format!("workspace-{workspace_id}")),
        status: Set("active".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    let job_id = Uuid::now_v7();
    yorishiro::edition::ee::models::inference_jobs::create(&db, job_id, workspace_id, "note")
        .await
        .unwrap();
    let lifecycle_id = Uuid::now_v7();
    yorishiro::models::queue_job_dispatch_outbox::record(
        &db,
        Enqueue {
            id: lifecycle_id,
            job_name: "infer_fill",
            worker_class: yorishiro::workers::embedding_sync::WorkerClass::Shared,
            workspace_id: Some(workspace_id),
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
        serde_json::json!({"job_id": job_id, "workspace_id": workspace_id, "schema_name":"note"}),
        InferFillWorker::class_name(),
        Some(WorkerTarget::Shared.queue()),
        Some(vec![WorkerTarget::Shared.tag()]),
    )
    .await
    .unwrap();
    let txn = db.begin().await.unwrap();
    yorishiro::models::queue_job_dispatch_outbox::record_lifecycle_payload(
        &txn,
        lifecycle_id,
        serde_json::json!({"job_id": job_id, "workspace_id": workspace_id, "schema_name":"note"}),
        InferFillWorker::class_name(),
        Some(WorkerTarget::Shared.queue()),
        Some(vec![WorkerTarget::Shared.tag()]),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    yorishiro::workers::dispatch::recover_pending_on(&ctx, &db).await;
    let queue_db = sea_orm::sqlx::SqlitePool::connect(&queue_uri)
        .await
        .unwrap();
    let queued =
        sea_orm::sqlx::query("SELECT name, task_data FROM sqlt_loco_queue WHERE status = 'queued'")
            .fetch_all(&queue_db)
            .await
            .unwrap();
    assert!(!queued.is_empty());
    let name: String = sea_orm::sqlx::Row::try_get(&queued[0], "name").unwrap();
    assert_eq!(name, InferFillWorker::class_name());

    let provider = queue.clone();
    let runner = tokio::spawn(async move { provider.run(vec![WorkerTarget::Shared.tag()]).await });
    tokio::time::timeout(Duration::from_secs(10), async {
            loop {
            let row: Option<(String, i64)> = sea_orm::sqlx::query_as(
                "SELECT status, COUNT(*) OVER () FROM sqlt_loco_queue WHERE name = ? ORDER BY created_at DESC LIMIT 1",
            )
            .bind(InferFillWorker::class_name())
            .fetch_optional(&queue_db)
            .await
            .unwrap();
            let Some((status, count)) = row else {
                panic!("relay job remains present until consumed");
            };
            if status == "completed" {
                break;
            }
            assert!(count > 0, "relay job remains present until consumed");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("registered InferFillWorker consumed the relay job");
    queue.shutdown().unwrap();
    runner.abort();
    let _ = runner.await;
    queue.clear().await.unwrap();
    queue_db.close().await;
    db.close().await.unwrap();
}

#[tokio::test]
async fn stalled_provider_enqueue_does_not_block_new_dispatch_records() {
    use loco_rs::bgworker::Queue;
    use yorishiro::models::_entities::queue_job_dispatch_outbox::Entity as Outbox;
    use yorishiro::models::queue_job_dispatch_outbox;
    use yorishiro::models::queue_job_lifecycles::Enqueue;
    use yorishiro::workers::embedding_sync::WorkerClass;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stalled-relay.sqlite3");
    let db = sea_orm::Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();
    let config = crate::sqlite_fixture_config(&format!("sqlite://{}?mode=rwc", path.display()));
    let provider = Arc::new(StalledQueue::default());
    let ctx = AppContext::builder(loco_rs::environment::Environment::Test, db.clone(), config)
        .build()
        .into_builder()
        .queue_provider(Arc::new(Queue::from_provider(provider.clone())))
        .build();

    let stalled_id = Uuid::now_v7();
    queue_job_dispatch_outbox::record(
        &db,
        Enqueue {
            id: stalled_id,
            job_name: "infer_fill",
            worker_class: WorkerClass::Shared,
            workspace_id: None,
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
        serde_json::json!({"job_id": Uuid::now_v7()}),
        "InferFillWorker".into(),
        Some("shared".into()),
        None,
    )
    .await
    .unwrap();
    let stalled_row = queue_job_dispatch_outbox::due(&db)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == stalled_id)
        .unwrap();
    let relay_ctx = ctx.clone();
    let relay = tokio::spawn(async move {
        yorishiro::workers::dispatch::recover_pending_on(&relay_ctx, &db).await;
    });
    tokio::time::timeout(Duration::from_secs(2), provider.entered.notified())
        .await
        .expect("relay reached the stalled provider enqueue");

    let producer_id = Uuid::now_v7();
    tokio::time::timeout(
        Duration::from_secs(2),
        queue_job_dispatch_outbox::record(
            &ctx.db,
            Enqueue {
                id: producer_id,
                job_name: "producer_probe",
                worker_class: WorkerClass::Shared,
                workspace_id: None,
                plan: None,
                concurrency_key: None,
                concurrency_limit: None,
            },
            serde_json::json!({"producer": true}),
            "ProducerProbeWorker".into(),
            None,
            None,
        ),
    )
    .await
    .expect("producer commits while provider enqueue remains stalled")
    .unwrap();

    provider.release.notify_one();
    relay.await.unwrap();
    let published = Outbox::find_by_id(stalled_row.id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    assert!(published.published_at.is_some());
    assert!(
        Outbox::find_by_id(producer_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .is_some()
    );
    ctx.db.close().await.unwrap();
}

#[tokio::test]
async fn zero_capacity_defers_lifecycle_and_rewrites_outbox_atomically() {
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    use yorishiro::edition::ee::models::inference_jobs::{self, InferenceJobStatus};
    use yorishiro::models::queue_job_lifecycles::{Enqueue, Entity as Lifecycles};
    use yorishiro::workers::embedding_sync::WorkerClass;
    use yorishiro::workers::target::WorkerTarget;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("infer-fill-saturated.sqlite3");
    let db = sea_orm::Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();
    let config = crate::sqlite_fixture_config(&format!("sqlite://{}?mode=rwc", path.display()));
    let ctx = AppContext::builder(
        loco_rs::environment::Environment::Test,
        db.clone(),
        config.clone(),
    )
    .build();
    ctx.shared_store.insert(
        config
            .settings::<yorishiro::data::settings::Settings>()
            .unwrap(),
    );
    let tenant_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
    yorishiro::models::_entities::tenant_tenants::ActiveModel {
        id: Set(tenant_id),
        name: Set(format!("tenant-{tenant_id}")),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    yorishiro::models::_entities::workspace_workspaces::ActiveModel {
        id: Set(workspace_id),
        tenant_id: Set(tenant_id),
        name: Set(format!("workspace-{workspace_id}")),
        status: Set("active".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    let job_id = Uuid::now_v7();
    inference_jobs::create(&db, job_id, workspace_id, "note")
        .await
        .unwrap();
    let lifecycle_id = Uuid::now_v7();
    yorishiro::models::queue_job_dispatch_outbox::record(&db, Enqueue {
        id: lifecycle_id,
        job_name: "infer_fill",
        worker_class: WorkerClass::Shared,
        workspace_id: Some(workspace_id),
        plan: None,
        concurrency_key: Some("capacity-zero"),
        concurrency_limit: Some(0),
    }, serde_json::json!({"job_id":job_id,"workspace_id":workspace_id,"schema_name":"note","dispatch_id":"stale"}), InferFillWorker::class_name(), Some(WorkerTarget::Shared.queue()), Some(vec![WorkerTarget::Shared.tag()])).await.unwrap();
    let worker = InferFillWorker::build(&ctx);
    let args = InferFillArgs {
        lifecycle_id: Some(lifecycle_id),
        job_id,
        workspace_id,
        schema_name: "note".into(),
    };
    worker.perform(args).await.unwrap();
    let lifecycle = Lifecycles::find_by_id(lifecycle_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let job = inference_jobs::get(&db, job_id).await.unwrap().unwrap();
    assert_eq!(lifecycle.status, "queued");
    assert_eq!(lifecycle.attempt, 0);
    assert_eq!(job.status, InferenceJobStatus::Queued);
    db.close().await.unwrap();
}

#[tokio::test]
async fn busy_workspace_defers_lifecycle_without_changing_inference_job() {
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    use yorishiro::edition::ee::models::inference_jobs::{self, InferenceJobStatus};
    use yorishiro::models::queue_job_lifecycles::{Enqueue, Entity as Lifecycles};
    use yorishiro::workers::embedding_sync::WorkerClass;
    use yorishiro::workers::target::WorkerTarget;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("infer-fill-busy.sqlite3");
    let db = sea_orm::Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();
    let config = crate::sqlite_fixture_config(&format!("sqlite://{}?mode=rwc", path.display()));
    let ctx = AppContext::builder(
        loco_rs::environment::Environment::Test,
        db.clone(),
        config.clone(),
    )
    .build();
    ctx.shared_store.insert(
        config
            .settings::<yorishiro::data::settings::Settings>()
            .unwrap(),
    );
    let tenant_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
    yorishiro::models::_entities::tenant_tenants::ActiveModel {
        id: Set(tenant_id),
        name: Set(format!("tenant-{tenant_id}")),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    yorishiro::models::_entities::workspace_workspaces::ActiveModel {
        id: Set(workspace_id),
        tenant_id: Set(tenant_id),
        name: Set(format!("workspace-{workspace_id}")),
        status: Set("active".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    let running_id = Uuid::now_v7();
    let deferred_id = Uuid::now_v7();
    inference_jobs::create(&db, running_id, workspace_id, "note")
        .await
        .unwrap();
    inference_jobs::create(&db, deferred_id, workspace_id, "note")
        .await
        .unwrap();
    let lifecycle_id = Uuid::now_v7();
    yorishiro::models::queue_job_dispatch_outbox::record(
        &db,
        Enqueue {
            id: lifecycle_id,
            job_name: "infer_fill",
            worker_class: WorkerClass::Shared,
            workspace_id: Some(workspace_id),
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
        serde_json::json!({"job_id":deferred_id,"workspace_id":workspace_id,"schema_name":"note"}),
        InferFillWorker::class_name(),
        Some(WorkerTarget::Shared.queue()),
        Some(vec![WorkerTarget::Shared.tag()]),
    )
    .await
    .unwrap();
    let mut running = yorishiro::models::_entities::inference_jobs::Entity::find_by_id(running_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    running.status = Set("running".into());
    running.attempt = Set(1);
    running.update(&db).await.unwrap();
    let worker = InferFillWorker::build(&ctx);
    worker
        .perform(InferFillArgs {
            lifecycle_id: Some(lifecycle_id),
            job_id: deferred_id,
            workspace_id,
            schema_name: "note".into(),
        })
        .await
        .unwrap();
    let lifecycle = Lifecycles::find_by_id(lifecycle_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let deferred = inference_jobs::get(&db, deferred_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lifecycle.status, "queued");
    assert_eq!(lifecycle.attempt, 0);
    assert_eq!(deferred.status, InferenceJobStatus::Queued);
    db.close().await.unwrap();
}

#[tokio::test]
async fn inference_failure_requeues_inference_lifecycle_and_outbox_together() {
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    use yorishiro::edition::ee::models::inference_jobs::{self, InferenceJobStatus};
    use yorishiro::models::queue_job_lifecycles::{Enqueue, Entity as Lifecycles};
    use yorishiro::workers::embedding_sync::WorkerClass;
    use yorishiro::workers::target::WorkerTarget;

    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("infer-fill-failure.sqlite3");
    let db = sea_orm::Database::connect(format!("sqlite://{}?mode=rwc", database_path.display()))
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();
    let config =
        crate::sqlite_fixture_config(&format!("sqlite://{}?mode=rwc", database_path.display()));
    let ctx = AppContext::builder(
        loco_rs::environment::Environment::Test,
        db.clone(),
        config.clone(),
    )
    .build();
    ctx.shared_store.insert(
        config
            .settings::<yorishiro::data::settings::Settings>()
            .unwrap(),
    );
    let tenant_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
    yorishiro::models::_entities::tenant_tenants::ActiveModel {
        id: Set(tenant_id),
        name: Set(format!("tenant-{tenant_id}")),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    yorishiro::models::_entities::workspace_workspaces::ActiveModel {
        id: Set(workspace_id),
        tenant_id: Set(tenant_id),
        name: Set(format!("workspace-{workspace_id}")),
        status: Set("active".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    let job_id = Uuid::now_v7();
    inference_jobs::create(&db, job_id, workspace_id, "note")
        .await
        .unwrap();
    let lifecycle_id = Uuid::now_v7();
    yorishiro::models::queue_job_dispatch_outbox::record(
        &db,
        Enqueue {
            id: lifecycle_id,
            job_name: "infer_fill",
            worker_class: WorkerClass::Shared,
            workspace_id: Some(workspace_id),
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
        serde_json::json!({"job_id":job_id,"workspace_id":workspace_id,"schema_name":"note"}),
        InferFillWorker::class_name(),
        Some(WorkerTarget::Shared.queue()),
        Some(vec![WorkerTarget::Shared.tag()]),
    )
    .await
    .unwrap();
    let worker = InferFillWorker::build(&ctx);
    worker
        .perform(InferFillArgs {
            lifecycle_id: Some(lifecycle_id),
            job_id,
            workspace_id,
            schema_name: "note".into(),
        })
        .await
        .unwrap();
    let lifecycle = Lifecycles::find_by_id(lifecycle_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let job = inference_jobs::get(&db, job_id).await.unwrap().unwrap();
    assert_eq!(lifecycle.status, "queued");
    assert_eq!(lifecycle.attempt, 0);
    assert_eq!(job.status, InferenceJobStatus::Queued);
    db.close().await.unwrap();
}

/// A chat-completions server that answers every request with one proposal after a pause, and records how many requests were in flight at once.
async fn slow_llm() -> (
    String,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind the fake LLM");
    let address = listener.local_addr().expect("fake LLM address");
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (task_flight, task_peak) = (in_flight.clone(), peak.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let (flight, peak) = (task_flight.clone(), task_peak.clone());
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                // The headers end the request line block; the body that follows is read to its declared length.
                loop {
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
                let now = flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                flight.fetch_sub(1, Ordering::SeqCst);
                let body = r#"{"choices":[{"message":{"content":"{\"summary\":\"proposed\"}"}}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    (format!("http://{address}"), in_flight, peak)
}

async fn gated_llm() -> (
    String,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Semaphore>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind gated fake LLM");
    let address = listener.local_addr().expect("gated fake LLM address");
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let task_entered = entered.clone();
    let task_release = release.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let (entered, release) = (task_entered.clone(), task_release.clone());
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n")
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
                entered.notify_one();
                let Ok(permit) = release.acquire().await else {
                    return;
                };
                permit.forget();
                let body = r#"{"choices":[{"message":{"content":"{\"summary\":\"old-schema-proposal\"}"}}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    (format!("http://{address}"), entered, release)
}

#[tokio::test]
async fn schema_change_during_llm_call_rejects_the_old_schema_proposal() {
    use yorishiro::edition::ee::models::{inference_jobs, inference_proposals};
    use yorishiro::models::queue_job_lifecycles::Enqueue;
    use yorishiro::workers::embedding_sync::WorkerClass;
    use yorishiro::workers::target::WorkerTarget;

    let (base_url, entered, release) = gated_llm().await;
    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        crate::requests::inference::licence(&ctx);
        let setup = crate::requests::inference::setup(&ctx).await;
        let _entity_id = crate::requests::inference::create_entity(&request, &setup).await;
        let first_update = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "name": "note",
                "entity_types": { "note": { "fields": {
                    "title": { "type": "string", "required": true },
                    "summary": { "type": "string" },
                    "extra": { "type": "string" }
                } } }
            }))
            .await;
        assert_eq!(first_update.status_code(), 201, "{}", first_update.text());
        let llm_key = request
            .put("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "base_url": base_url,
                "model": "test-model",
                "api_key": "sk-test"
            }))
            .await;
        assert_eq!(llm_key.status_code(), 204, "{}", llm_key.text());

        let job_id = Uuid::now_v7();
        inference_jobs::create(&ctx.db, job_id, setup.workspace_id, "note")
            .await
            .expect("create infer-fill job");
        let lifecycle_id = Uuid::now_v7();
        yorishiro::models::queue_job_dispatch_outbox::record(
            &ctx.db,
            Enqueue {
                id: lifecycle_id,
                job_name: "infer_fill",
                worker_class: WorkerClass::Shared,
                workspace_id: Some(setup.workspace_id),
                plan: None,
                concurrency_key: None,
                concurrency_limit: None,
            },
            serde_json::json!({
                "job_id": job_id,
                "workspace_id": setup.workspace_id,
                "schema_name": "note"
            }),
            InferFillWorker::class_name(),
            Some(WorkerTarget::Shared.queue()),
            Some(vec![WorkerTarget::Shared.tag()]),
        )
        .await
        .expect("record infer-fill delivery");
        let worker = InferFillWorker::build(&ctx);
        let run = tokio::spawn(async move {
            worker
                .perform(InferFillArgs {
                    lifecycle_id: Some(lifecycle_id),
                    job_id,
                    workspace_id: setup.workspace_id,
                    schema_name: "note".into(),
                })
                .await
        });

        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .expect("worker reached the external LLM call");
        let replacement = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "name": "note",
                "entity_types": { "note": { "fields": {
                    "title": { "type": "string", "required": true },
                    "summary": { "type": "string" },
                    "extra": { "type": "string" },
                    "later": { "type": "string" }
                } } }
            }))
            .await;
        assert_eq!(replacement.status_code(), 201, "{}", replacement.text());

        release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("worker finishes after the LLM reply")
            .expect("join infer-fill worker")
            .expect("schema-change retry is durably deferred");

        let proposals = inference_proposals::for_job(&ctx.db, setup.workspace_id, job_id)
            .await
            .expect("read proposals");
        assert!(
            proposals.is_empty(),
            "stale schema proposals were written: {proposals:?}"
        );
        let job = inference_jobs::get(&ctx.db, job_id)
            .await
            .expect("read inference job")
            .expect("inference job exists");
        assert_eq!(job.status, inference_jobs::InferenceJobStatus::Queued);

        // A reclaimed inference attempt must not accept the first attempt's late LLM answer.
        let entity = request
            .post("/api/entities")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "schema_name": "note",
                "entity_type": "note",
                "data": { "title": "second entity" }
            }))
            .await;
        assert_eq!(entity.status_code(), 201, "{}", entity.text());
        let _entity_id: Uuid = entity.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("entity id")
            .parse()
            .expect("UUID entity id");
        let reclaimed_job_id = Uuid::now_v7();
        inference_jobs::create(&ctx.db, reclaimed_job_id, setup.workspace_id, "note")
            .await
            .expect("create reclaim regression job");
        let reclaimed_lifecycle_id = Uuid::now_v7();
        yorishiro::models::queue_job_dispatch_outbox::record(
            &ctx.db,
            Enqueue {
                id: reclaimed_lifecycle_id,
                job_name: "infer_fill",
                worker_class: WorkerClass::Shared,
                workspace_id: Some(setup.workspace_id),
                plan: None,
                concurrency_key: None,
                concurrency_limit: None,
            },
            serde_json::json!({
                "job_id": reclaimed_job_id,
                "workspace_id": setup.workspace_id,
                "schema_name": "note"
            }),
            InferFillWorker::class_name(),
            Some(WorkerTarget::Shared.queue()),
            Some(vec![WorkerTarget::Shared.tag()]),
        )
        .await
        .expect("record reclaim regression delivery");
        let reclaimed_worker = InferFillWorker::build(&ctx);
        let reclaimed_run = tokio::spawn(async move {
            reclaimed_worker
                .perform(InferFillArgs {
                    lifecycle_id: Some(reclaimed_lifecycle_id),
                    job_id: reclaimed_job_id,
                    workspace_id: setup.workspace_id,
                    schema_name: "note".into(),
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .expect("reclaimed job reached the external LLM call");
        assert!(
            inference_jobs::fail_attempt(&ctx.db, reclaimed_job_id, 1, "test reclaim")
                .await
                .expect("retire original attempt")
        );
        assert!(
            inference_jobs::reconcile_attempt(&ctx.db, reclaimed_job_id, 2)
                .await
                .expect("admit replacement attempt")
        );
        release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(5), reclaimed_run)
            .await
            .expect("stale LLM reply is rejected")
            .expect("join stale infer-fill worker")
            .expect("stale attempt is durably deferred");
        let stale_proposals =
            inference_proposals::for_job(&ctx.db, setup.workspace_id, reclaimed_job_id)
                .await
                .expect("read reclaim proposals");
        assert!(
            stale_proposals.is_empty(),
            "stale attempt proposals were written: {stale_proposals:?}"
        );
        let replacement = inference_jobs::get(&ctx.db, reclaimed_job_id)
            .await
            .expect("read replacement attempt")
            .expect("replacement inference job exists");
        assert_eq!(
            replacement.status,
            inference_jobs::InferenceJobStatus::Running
        );
        assert_eq!(replacement.attempt, 2);
    })
    .await;
}

/// Two jobs for one workspace never run together, whichever backend: PostgreSQL holds an advisory lock, and SQLite, whose lock is a no-op, relies on the one-running-job-per-workspace index.
/// The job refused while the other runs makes no LLM call and succeeds once redelivered.
#[tokio::test]
#[serial_test::serial(postgres_sqlite_queue)]
async fn two_jobs_for_one_workspace_never_run_together() {
    use std::sync::atomic::Ordering;
    use yorishiro::edition::ee::models::inference_jobs::{self, InferenceJobStatus};
    use yorishiro::edition::ee::models::inference_proposals;
    use yorishiro::models::queue_job_lifecycles::Enqueue;
    use yorishiro::workers::target::WorkerTarget;

    let (base_url, in_flight, peak) = slow_llm().await;
    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        crate::requests::inference::licence(&ctx);
        let setup = crate::requests::inference::setup(&ctx).await;
        crate::requests::inference::create_entity(&request, &setup).await;
        // A second version leaves the entity on an outdated schema, which is what infer-fill walks.
        let next = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "name": "note",
                "entity_types": { "note": { "fields": {
                    "title": { "type": "string", "required": true },
                    "summary": { "type": "string" },
                    "extra": { "type": "string" }
                } } }
            }))
            .await;
        assert_eq!(next.status_code(), 201, "{:?}", next.text());
        let key = request
            .put("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "base_url": base_url,
                "model": "test-model",
                "api_key": "sk-test"
            }))
            .await;
        assert_eq!(key.status_code(), 204, "{:?}", key.text());

        let jobs = [Uuid::new_v4(), Uuid::new_v4()];
        for job_id in jobs {
            inference_jobs::create(&ctx.db, job_id, setup.workspace_id, "note")
                .await
                .expect("create job");
        }
        let args = |job_id, lifecycle_id| InferFillArgs {
            lifecycle_id: Some(lifecycle_id),
            job_id,
            workspace_id: setup.workspace_id,
            schema_name: "note".into(),
        };
        let worker = InferFillWorker::build(&ctx);
        let deliveries = [Uuid::now_v7(), Uuid::now_v7()];
        for index in 0..jobs.len() {
            let job_id = jobs[index];
            let lifecycle_id = deliveries[index];
            let enqueue = Enqueue {
                id: lifecycle_id,
                job_name: "infer_fill",
                worker_class: yorishiro::workers::embedding_sync::WorkerClass::Shared,
                workspace_id: Some(setup.workspace_id),
                plan: None,
                concurrency_key: None,
                concurrency_limit: None,
            };
            yorishiro::models::queue_job_dispatch_outbox::record(
                &ctx.db,
                enqueue,
                serde_json::json!({"job_id": job_id, "workspace_id": setup.workspace_id, "schema_name":"note"}),
                InferFillWorker::class_name(),
                Some(WorkerTarget::Shared.queue()),
                Some(vec![WorkerTarget::Shared.tag()]),
            )
            .await
            .expect("record current-epoch delivery");
            let claim = Uuid::now_v7();
            assert!(yorishiro::models::queue_job_dispatch_outbox::claim_dispatch(
                &ctx.db,
                lifecycle_id,
                claim,
                30,
            )
            .await
            .expect("claim fixture delivery"));
            assert!(yorishiro::models::queue_job_dispatch_outbox::acknowledge_dispatch(
                &ctx.db,
                lifecycle_id,
                claim,
                &format!("fixture-provider-job-{lifecycle_id}"),
            )
            .await
            .expect("acknowledge fixture delivery"));
        }
        let (first, second) = tokio::join!(
            worker.perform(args(jobs[0], deliveries[0])),
            worker.perform(args(jobs[1], deliveries[1]))
        );
        assert!(first.is_ok() && second.is_ok(), "{first:?} / {second:?}");
        assert_eq!(peak.load(Ordering::SeqCst), 1, "LLM calls overlapped");
        assert_eq!(in_flight.load(Ordering::SeqCst), 0);

        for index in 0..jobs.len() {
            let job_id = jobs[index];
            let lifecycle_id = deliveries[index];
            let lifecycle = yorishiro::models::queue_job_lifecycles::Entity::find_by_id(lifecycle_id)
                .one(&ctx.db)
                .await
                .expect("read lifecycle")
                .expect("lifecycle exists");
            if lifecycle.status == "queued" {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                worker
                    .perform(args(job_id, lifecycle_id))
                    .await
                    .expect("deferred delivery runs once workspace is free");
            }
        }

        assert_eq!(peak.load(Ordering::SeqCst), 1, "LLM calls overlapped");
        assert_eq!(in_flight.load(Ordering::SeqCst), 0);
        for job_id in jobs {
            let job = inference_jobs::get(&ctx.db, job_id)
                .await
                .expect("read job")
                .expect("job exists");
            assert_eq!(job.status, InferenceJobStatus::Completed, "{job:?}");
            let proposals = inference_proposals::for_job(&ctx.db, setup.workspace_id, job_id)
                .await
                .expect("read proposals");
            assert_eq!(proposals.len(), 1, "one proposal per job: {proposals:?}");
        }
    })
    .await;
}
