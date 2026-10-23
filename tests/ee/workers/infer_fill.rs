use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::*;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use yorishiro::edition::ee::workers::infer_fill::*;

#[tokio::test]
async fn rejects_a_missing_queue_before_creating_a_job() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, mut ctx| async move {
        ctx.queue_provider = None;

        let error = enqueue_infer_fill(&ctx, Uuid::now_v7(), "note".into())
            .await
            .expect_err("missing queue must fail");

        assert_eq!(
            error.to_string(),
            "infer-fill requires a queue provider (configure queue: in the server config)"
        );
    })
    .await;
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
    use futures::FutureExt;
    use loco_rs::{app::Hooks, bgworker::JobStatus};
    use yorishiro::workers::target::WorkerTarget;

    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let workspace_id = fixture.workspace_id;
        let db = ctx.db.clone();
        let target = WorkerTarget::Shared;
        let queue = crate::requests::query_worker::target_queue(&ctx, target).await;
        let ctx = ctx.into_builder().queue_provider(queue.clone()).build();
        yorishiro::App::connect_workers(&ctx, &queue).await.unwrap();
        let job_id = enqueue_infer_fill(&ctx, workspace_id, "note".into())
            .await
            .unwrap();
        let job_id = Uuid::parse_str(&job_id).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let relayed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let dump = queue.dump(directory.path(), None, None).await.unwrap();
                let queued: Vec<loco_rs::bgworker::Job> =
                    serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                if let Some(job) = queued
                    .into_iter()
                    .find(|job| job.data["job_id"] == job_id.to_string())
                {
                    break job;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("production relay publishes this lifecycle");
        assert_eq!(relayed.name, InferFillWorker::class_name());
        assert_eq!(relayed.status, JobStatus::Queued);
        assert_eq!(relayed.tags, Some(vec![target.tag()]));
        let provider = queue.clone();
        let runner = tokio::spawn(async move { provider.run(vec![target.tag()]).await });
        let result =
            std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let job = yorishiro::edition::ee::models::inference_jobs::get(&db, job_id)
                        .await
                        .unwrap()
                        .expect("inference job remains until consumed");
                    if job.attempt > 0 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }))
            .catch_unwind()
            .await;
        yorishiro::App::on_shutdown(&ctx).await;
        queue.shutdown().unwrap();
        tokio::time::timeout(Duration::from_secs(10), runner)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match result {
            Ok(Ok(())) => {
                let dump = queue.dump(directory.path(), None, None).await.unwrap();
                let jobs: Vec<loco_rs::bgworker::Job> =
                    serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                queue.close().await.unwrap();
                let job = jobs
                    .iter()
                    .find(|job| job.data["job_id"] == job_id.to_string())
                    .expect("relay job remains after consumption");
                assert_eq!(job.status, JobStatus::Completed);
            }
            Ok(Err(error)) => {
                queue.close().await.unwrap();
                panic!("registered InferFillWorker consumed the relay job: {error}");
            }
            Err(panic) => {
                queue.close().await.unwrap();
                std::panic::resume_unwind(panic);
            }
        }
    })
    .await;
}

#[tokio::test]
async fn stalled_provider_enqueue_does_not_block_new_dispatch_records() {
    use futures::FutureExt;
    use yorishiro::models::_entities::queue_job_dispatch_outbox::Entity as Outbox;

    if !crate::require_sqlite_backend() {
        return;
    }
    // Lock only the owned queue file, leaving the producer database writable.
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let Some(loco_rs::config::QueueConfig::Sqlite(config)) = ctx.config.queue.as_ref() else {
            return;
        };
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        use sea_orm::sqlx::{Connection, sqlite::SqliteConnection};
        let mut blocker = SqliteConnection::connect(&config.uri).await.unwrap();
        sea_orm::sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut blocker)
            .await
            .unwrap();

        let stalled_job = enqueue_infer_fill(&ctx, fixture.workspace_id, "note".into())
            .await
            .unwrap();
        let stalled_id = Outbox::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.payload["job_id"] == stalled_job)
            .unwrap()
            .id;
        let relay_ctx = ctx.clone();
        let relay = tokio::spawn(async move {
            yorishiro::workers::dispatch::recover_pending(&relay_ctx).await;
        });
        let mut producer_job = None;
        let result = std::panic::AssertUnwindSafe(async {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let row = Outbox::find_by_id(stalled_id)
                        .one(&ctx.db)
                        .await
                        .unwrap()
                        .unwrap();
                    if row.claim_token.is_some() {
                        assert!(row.published_at.is_none());
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("relay claims the dispatch while the real queue is locked");

            producer_job = Some(
                tokio::time::timeout(
                    Duration::from_secs(2),
                    enqueue_infer_fill(&ctx, fixture.workspace_id, "next-note".into()),
                )
                .await
                .expect("producer commits while provider enqueue remains stalled")
                .unwrap(),
            );
        })
        .catch_unwind()
        .await;

        sea_orm::sqlx::query("ROLLBACK")
            .execute(&mut blocker)
            .await
            .unwrap();
        blocker.close().await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), relay)
            .await
            .unwrap()
            .unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
        let published = Outbox::find_by_id(stalled_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert!(published.published_at.is_some());
        let producer_job = producer_job.unwrap();
        assert!(
            Outbox::find()
                .all(&ctx.db)
                .await
                .unwrap()
                .iter()
                .any(|row| row.payload["job_id"] == producer_job)
        );
    })
    .await;
}

async fn deliver_lifecycle(ctx: &loco_rs::app::AppContext, lifecycle_id: Uuid) {
    use futures::FutureExt;
    use loco_rs::{app::Hooks, bgworker::JobStatus};
    use yorishiro::workers::target::WorkerTarget;

    let target = WorkerTarget::Shared;
    let queue = crate::requests::query_worker::target_queue(ctx, target).await;
    let worker_ctx = ctx
        .clone()
        .into_builder()
        .queue_provider(queue.clone())
        .build();
    yorishiro::App::connect_workers(&worker_ctx, &queue)
        .await
        .unwrap();
    let running = queue.clone();
    let worker = tokio::spawn(async move { running.run(vec![target.tag()]).await });
    let directory = tempfile::tempdir().unwrap();
    let outcome =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let dump = queue.dump(directory.path(), None, None).await.unwrap();
                let jobs: Vec<loco_rs::bgworker::Job> =
                    serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                if jobs.iter().any(|job| {
                    job.data["lifecycle_id"] == lifecycle_id.to_string()
                        && job.status == JobStatus::Completed
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }))
        .catch_unwind()
        .await;
    yorishiro::App::on_shutdown(&worker_ctx).await;
    queue.shutdown().unwrap();
    let drained = tokio::time::timeout(Duration::from_secs(10), worker).await;
    queue.close().await.unwrap();
    match outcome {
        Ok(result) => result.expect("real queue completes the lifecycle delivery"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
    drained.unwrap().unwrap().unwrap();
}

/// Holds responses until released, recording overlapping requests without timing assumptions.
async fn controlled_llm() -> (
    crate::http_stub::HttpStub,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Semaphore>,
) {
    use axum::{Json, Router, routing::post};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (task_entered, task_release) = (entered.clone(), release.clone());
    let (task_flight, task_peak) = (in_flight.clone(), peak.clone());
    let router = Router::new().route("/chat/completions", post(move || {
            let (flight, peak) = (task_flight.clone(), task_peak.clone());
            let (entered, release) = (task_entered.clone(), task_release.clone());
        async move {
            let now = flight.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
                entered.notify_one();
                release.acquire().await.expect("release LLM response").forget();
            flight.fetch_sub(1, Ordering::SeqCst);
            Json(serde_json::json!({"choices":[{"message":{"content":"{\"summary\":\"proposed\"}"}}]}))
        }
    }));
    (
        crate::http_stub::HttpStub::start(router).await,
        in_flight,
        peak,
        entered,
        release,
    )
}

async fn gated_llm() -> (
    crate::http_stub::HttpStub,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Semaphore>,
) {
    use axum::{Json, Router, routing::post};

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let task_entered = entered.clone();
    let task_release = release.clone();
    let router = Router::new().route("/chat/completions", post(move || {
        let (entered, release) = (task_entered.clone(), task_release.clone());
        async move {
            entered.notify_one();
            release.acquire().await.expect("release gated LLM").forget();
            Json(serde_json::json!({"choices":[{"message":{"content":"{\"summary\":\"old-schema-proposal\"}"}}]}))
        }
    }));
    (
        crate::http_stub::HttpStub::start(router).await,
        entered,
        release,
    )
}

#[tokio::test]
async fn schema_change_during_llm_call_rejects_the_old_schema_proposal() {
    use yorishiro::edition::ee::models::{inference_jobs, inference_proposals};

    let (llm, entered, release) = gated_llm().await;
    let base_url = llm.url.clone();
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let setup = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs::default(),
        )
        .await;
        let _entity_id = crate::requests::inference::create_entity(&request, &setup).await;
        let first_update = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": base_url,
                "model": "test-model",
                "api_key": "sk-test"
            }))
            .await;
        assert_eq!(llm_key.status_code(), 204, "{}", llm_key.text());

        let job_id = enqueue_infer_fill(&ctx, setup.workspace_id, "note".into())
            .await
            .unwrap()
            .parse::<Uuid>()
            .unwrap();
        let lifecycle_id = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.payload["job_id"] == job_id.to_string())
            .unwrap()
            .id;
        let delivery_ctx = ctx.clone();
        let run = tokio::spawn(async move {
            deliver_lifecycle(&delivery_ctx, lifecycle_id).await;
        });

        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .expect("worker reached the external LLM call");
        let replacement = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
            .expect("schema-change retry is durably deferred by the real worker");

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
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
        let reclaimed_job_id = enqueue_infer_fill(&ctx, setup.workspace_id, "note".into())
            .await
            .unwrap()
            .parse::<Uuid>()
            .unwrap();
        let reclaimed_lifecycle_id =
            yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
                .all(&ctx.db)
                .await
                .unwrap()
                .into_iter()
                .find(|row| row.payload["job_id"] == reclaimed_job_id.to_string())
                .unwrap()
                .id;
        let delivery_ctx = ctx.clone();
        let reclaimed_run = tokio::spawn(async move {
            deliver_lifecycle(&delivery_ctx, reclaimed_lifecycle_id).await;
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
            .expect("stale attempt is fenced by the real worker");
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
async fn two_jobs_for_one_workspace_never_run_together() {
    use futures::FutureExt;
    use loco_rs::{app::Hooks, bgworker::JobStatus};
    use std::sync::atomic::Ordering;
    use yorishiro::edition::ee::models::inference_jobs::{self, InferenceJobStatus};
    use yorishiro::edition::ee::models::inference_proposals;
    use yorishiro::workers::target::WorkerTarget;

    let (llm, in_flight, peak, entered, release) = controlled_llm().await;
    let base_url = llm.url.clone();
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let setup = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs::default(),
        )
        .await;
        crate::requests::inference::create_entity(&request, &setup).await;
        // A second version leaves the entity on an outdated schema, which is what infer-fill walks.
        let next = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": base_url,
                "model": "test-model",
                "api_key": "sk-test"
            }))
            .await;
        assert_eq!(key.status_code(), 204, "{:?}", key.text());

        let mut jobs = Vec::new();
        for _ in 0..2 {
            jobs.push(
                enqueue_infer_fill(&ctx, setup.workspace_id, "note".into())
                    .await
                    .expect("enqueue competing inference job")
                    .parse::<Uuid>()
                    .unwrap(),
            );
        }
        let jobs = [jobs[0], jobs[1]];
        // Two real consumers must contend while the first external call is held.
        let mut worker_ctx = ctx.clone();
        match worker_ctx.config.queue.as_mut().unwrap() {
            loco_rs::config::QueueConfig::Sqlite(queue) => queue.num_workers = 2,
            loco_rs::config::QueueConfig::Postgres(queue) => queue.num_workers = 2,
            loco_rs::config::QueueConfig::Redis(queue) => queue.num_workers = 2,
            _ => unreachable!("supported queue topology"),
        }
        let queue = crate::requests::query_worker::shared_target_queue(&worker_ctx).await;
        worker_ctx.queue_provider = Some(queue.clone());
        yorishiro::App::connect_workers(&worker_ctx, &queue)
            .await
            .unwrap();
        let running = queue.clone();
        let runner =
            tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
        let directory = tempfile::tempdir().unwrap();
        let result =
            std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(10), async {
                entered.notified().await;
                loop {
                    let dump = queue.dump(directory.path(), None, None).await.unwrap();
                    let queued: Vec<loco_rs::bgworker::Job> =
                        serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                    if queued.iter().any(|job| {
                        jobs.iter().any(|id| job.data["job_id"] == id.to_string())
                            && job.status == JobStatus::Completed
                    }) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                assert_eq!(
                    in_flight.load(Ordering::SeqCst),
                    1,
                    "the first LLM call is still held"
                );
                release.add_permits(2);
                loop {
                    let mut completed = 0;
                    for id in jobs {
                        if inference_jobs::get(&ctx.db, id)
                            .await
                            .unwrap()
                            .unwrap()
                            .status
                            == InferenceJobStatus::Completed
                        {
                            completed += 1;
                        }
                    }
                    if completed == jobs.len() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }))
            .catch_unwind()
            .await;
        release.add_permits(2);
        yorishiro::App::on_shutdown(&worker_ctx).await;
        queue.shutdown().unwrap();
        let drained = tokio::time::timeout(Duration::from_secs(10), runner).await;
        queue.close().await.unwrap();
        match result {
            Ok(result) => {
                result.expect("competing delivery defers and is redelivered by the real queue")
            }
            Err(panic) => std::panic::resume_unwind(panic),
        }
        drained.unwrap().unwrap().unwrap();

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
