use std::sync::Arc;

use futures::FutureExt;
use loco_rs::app::{AppContext, Hooks};
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::bgworker::JobStatus;
use sea_orm::EntityTrait;
use std::time::Duration;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::query_embedding_requests::{Entity as Requests, QueryOutcome};
use yorishiro::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use yorishiro::workers::query_embedding::{QueryEmbeddingArgs, QueryEmbeddingWorker};
use yorishiro::workers::target::WorkerTarget;

use crate::requests::boot_request_isolated as boot_request;

async fn workspace(request: &loco_rs::TestServer, ctx: &AppContext) -> Uuid {
    crate::requests::fixtures::TestTenant::create(request, ctx, Default::default())
        .await
        .workspace_id
}

async fn provider() -> (
    Arc<dyn EmbeddingProvider>,
    crate::http_stub::HttpStub,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = calls.clone();
    let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let calls = observed.clone();
            async move {
                assert_eq!(body["model"], "query-http-test");
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let data = body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .map(|(index, text)| {
                        let mut vector = vec![0.0_f32; 768];
                        vector[usize::from(text.as_str().unwrap().contains("beta"))] = 1.0;
                        serde_json::json!({"index": index, "embedding": vector})
                    })
                    .collect::<Vec<_>>();
                axum::Json(serde_json::json!({"data": data}))
            }
        }),
    ))
    .await;
    let provider = Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "test-key".into(),
        model: "query-http-test".into(),
        dimensions: 768,
        send_dimensions_param: false,
    }));
    (provider, server, calls)
}

async fn deliver_stale_request(ctx: &AppContext, workspace_id: Uuid, request_id: Uuid) {
    let mut worker_ctx = ctx.clone();
    let queue = crate::requests::query_worker::shared_target_queue(&mut worker_ctx).await;
    // Inject a late provider delivery after its caller's request is gone or expired.
    let provider_id = queue
        .enqueue(
            QueryEmbeddingWorker::class_name(),
            Some(WorkerTarget::Shared.queue()),
            serde_json::to_value(QueryEmbeddingArgs {
                request_id,
                workspace_id,
            })
            .unwrap(),
            Some(vec![WorkerTarget::Shared.tag()]),
            None,
        )
        .await
        .unwrap()
        .expect("provider returns the injected delivery ID");
    let running = queue.clone();
    let worker = tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
    let directory = tempfile::tempdir().unwrap();
    let outcome =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let dump = queue.dump(directory.path(), None, None).await.unwrap();
                let jobs: Vec<loco_rs::bgworker::Job> =
                    serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                if jobs
                    .iter()
                    .any(|job| job.id == provider_id && job.status == JobStatus::Completed)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }))
        .catch_unwind()
        .await;
    App::on_shutdown(&worker_ctx).await;
    queue.shutdown().unwrap();
    let drained = tokio::time::timeout(Duration::from_secs(10), worker).await;
    queue.close().await.unwrap();
    match outcome {
        Ok(result) => result.expect("real queue finishes the query delivery"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
    drained.unwrap().unwrap().unwrap();
}

/// Search jobs travel to the shared target when they are enqueued, so the worker declares no tag or queue of its own.
#[test]
fn the_query_worker_declares_no_routing_of_its_own() {
    assert!(QueryEmbeddingWorker::tags().is_empty());
    assert!(QueryEmbeddingWorker::queue().is_none());
}

/// The job names a request and a workspace and nothing else, so no query text sits in the queue.
#[test]
fn the_job_payload_carries_ids_only() {
    let args = QueryEmbeddingArgs {
        request_id: Uuid::nil(),
        workspace_id: Uuid::nil(),
    };
    let json = serde_json::to_value(&args).unwrap();
    let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["request_id", "workspace_id"]);
}

#[tokio::test]
async fn a_job_for_a_request_that_is_gone_or_finished_does_nothing() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let ws = workspace(&request, &ctx).await;
        let (provider, _server, calls) = provider().await;
        ctx.shared_store
            .insert(provider.clone() as Arc<dyn EmbeddingProvider>);

        deliver_stale_request(&ctx, ws, Uuid::now_v7()).await;

        let expired = Requests::open(
            &ctx.db,
            ws,
            "alpha",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
        assert!(Requests::expire(&ctx.db, ws, expired).await.unwrap());
        deliver_stale_request(&ctx, ws, expired).await;
        assert!(
            calls.load(std::sync::atomic::Ordering::SeqCst) == 0,
            "no embedding work was done for either"
        );
        assert_eq!(
            Requests::take(&ctx.db, ws, expired).await.unwrap(),
            Some(QueryOutcome::Expired)
        );
    })
    .await;
}

#[tokio::test]
async fn the_worker_stores_a_vector_of_the_width_the_workspace_holds() {
    let (provider, _server, _) = provider().await;
    crate::requests::query_worker::boot_with_query_worker(provider, |request, ctx| async move {
        let ws = workspace(&request, &ctx).await;
        let tenant = yorishiro::models::_entities::workspace_workspaces::Entity::find_by_id(ws)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap()
            .tenant_id;
        let vector = yorishiro::workers::query_embedding::embed_query(&ctx, tenant, ws, "beta")
            .await
            .unwrap();
        assert_eq!(vector.len(), 768);
        assert_eq!(vector[1], 1.0, "beta is the second axis");
    })
    .await;
}

#[tokio::test]
async fn a_failure_is_recorded_on_the_request_and_returned_to_loco() {
    let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(|| async {
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                [("Retry-After", "3")],
                "model is warming up",
            )
        }),
    ))
    .await;
    let provider = Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "test-key".into(),
        model: "test-model".into(),
        dimensions: 768,
        send_dimensions_param: false,
    })) as Arc<dyn EmbeddingProvider>;
    crate::requests::query_worker::boot_with_query_worker(provider, |request, ctx| async move {
        let ws = workspace(&request, &ctx).await;
        let tenant = yorishiro::models::_entities::workspace_workspaces::Entity::find_by_id(ws)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap()
            .tenant_id;
        let error = yorishiro::workers::query_embedding::embed_query(&ctx, tenant, ws, "alpha")
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("HTTP 503"), "{message}");
        assert!(
            !message.contains("model is warming up"),
            "provider response bodies must stay private"
        );
        let directory = tempfile::tempdir().unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let dump = ctx
                    .queue_provider
                    .as_ref()
                    .unwrap()
                    .dump(directory.path(), None, None)
                    .await
                    .unwrap();
                let jobs: Vec<loco_rs::bgworker::Job> =
                    serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                if let Some(job) = jobs.iter().find(|job| {
                    job.data["workspace_id"] == ws.to_string()
                        && job.name == QueryEmbeddingWorker::class_name()
                        && job.status == JobStatus::Failed
                }) {
                    let diagnostic = job.data.to_string();
                    assert!(diagnostic.contains("HTTP 503"));
                    assert!(!diagnostic.contains("model is warming up"));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("Loco records the query delivery failure");
    })
    .await;
}
