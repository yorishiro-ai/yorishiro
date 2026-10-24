//! Regression tests for the upstream defects documented in patches/loco-rs/README.md.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use loco_rs::bgworker::BackgroundWorker;
use redis::AsyncCommands;
use serde_json::{Value, json};
use tokio::sync::mpsc;

#[tokio::test]
async fn sql_queue_close_releases_pool_and_retained_worker_context() {
    use loco_rs::app::Hooks;

    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        if matches!(
            ctx.config.queue,
            Some(loco_rs::config::QueueConfig::Redis(_))
        ) {
            return;
        }
        let queue = crate::requests::query_worker::shared_target_queue(&ctx).await;
        let mut worker_ctx = ctx.clone();
        worker_ctx.queue_provider = Some(queue.clone());
        yorishiro::App::connect_workers(&worker_ctx, &queue)
            .await
            .unwrap();
        queue.ping().await.unwrap();
        yorishiro::App::on_shutdown(&worker_ctx).await;
        drop(worker_ctx);
        assert!(
            Arc::strong_count(&queue) > 1,
            "production handlers retain their queue context"
        );
        tokio::time::timeout(Duration::from_secs(5), queue.close())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            Arc::strong_count(&queue),
            1,
            "close releases every retained worker context"
        );
        assert!(queue.ping().await.is_err(), "close awaits the SQL pool");
    })
    .await;
}

#[tokio::test]
async fn redis_claim_refreshes_age_and_fences_reaped_completion_and_failure() {
    use futures::FutureExt;
    use loco_rs::app::Hooks;
    use yorishiro::models::query_embedding_requests::Entity as Requests;
    use yorishiro::services::embedding::{
        EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
    };
    use yorishiro::workers::query_embedding::{QueryEmbeddingArgs, QueryEmbeddingWorker};
    use yorishiro::workers::target::WorkerTarget;

    for fail_first in [false, true] {
        crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
            if !matches!(
                ctx.config.queue,
                Some(loco_rs::config::QueueConfig::Redis(_))
            ) {
                return;
            }
            let fixture =
                crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default())
                    .await;
            let attempt = Arc::new(AtomicUsize::new(0));
            let release = [
                Arc::new(tokio::sync::Semaphore::new(0)),
                Arc::new(tokio::sync::Semaphore::new(0)),
            ];
            let (started, mut starts) = mpsc::unbounded_channel();
            let observed = attempt.clone();
            let gates = release.clone();
            let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
                "/embeddings",
                axum::routing::post(move || {
                    let attempt = observed.fetch_add(1, Ordering::SeqCst);
                    let started = started.clone();
                    let gate = gates[attempt].clone();
                    async move {
                        started.send(attempt).unwrap();
                        gate.acquire().await.unwrap().forget();
                        if attempt == 0 && fail_first {
                            (
                                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                                axum::Json(json!({"error":"private upstream body"})),
                            )
                        } else {
                            (
                                axum::http::StatusCode::OK,
                                axum::Json(
                                    json!({"data":[{"index":0,"embedding":vec![1.0_f32;768]}]}),
                                ),
                            )
                        }
                    }
                }),
            ))
            .await;
            ctx.shared_store
                .insert(
                    Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                        base_url: server.url.clone(),
                        api_key: "test-key".into(),
                        model: "claim-fencing".into(),
                        dimensions: 768,
                        send_dimensions_param: false,
                    })) as Arc<dyn EmbeddingProvider>,
                );
            let first = crate::requests::query_worker::shared_target_queue(&ctx).await;
            let second = crate::requests::query_worker::shared_target_queue(&ctx).await;
            let mut worker_ctx = ctx.clone();
            worker_ctx.queue_provider = Some(first.clone());
            yorishiro::App::connect_workers(&worker_ctx, &first)
                .await
                .unwrap();
            let mut second_ctx = ctx.clone();
            second_ctx.queue_provider = Some(second.clone());
            yorishiro::App::connect_workers(&second_ctx, &second)
                .await
                .unwrap();
            let request_id = Requests::open(
                &ctx.db,
                fixture.workspace_id,
                "claim age",
                600,
                chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(5),
            )
            .await
            .unwrap();
            let id = first
                .enqueue(
                    QueryEmbeddingWorker::class_name(),
                    Some(WorkerTarget::Shared.queue()),
                    QueryEmbeddingArgs {
                        request_id,
                        workspace_id: fixture.workspace_id,
                    },
                    Some(vec![WorkerTarget::Shared.tag()]),
                    None,
                )
                .await
                .unwrap()
                .unwrap();
            let uri = match ctx.config.queue.as_ref().unwrap() {
                loco_rs::config::QueueConfig::Redis(config) => &config.uri,
                _ => panic!("Redis lane must configure Redis"),
            };
            let mut conn = redis::Client::open(uri.as_str())
                .unwrap()
                .get_multiplexed_async_connection()
                .await
                .unwrap();
            let key = format!("job:{id}");
            let mut old: Value =
                serde_json::from_str(&conn.get::<_, String>(&key).await.unwrap()).unwrap();
            // Fault injection affects only this delivery's age, never shared queue membership.
            old["updated_at"] = json!("2000-01-01T00:00:00Z");
            conn.set::<_, _, ()>(&key, old.to_string()).await.unwrap();
            let running = first.clone();
            let first_run =
                tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
            let mut second_run = None;
            let outcome = std::panic::AssertUnwindSafe(async {
                assert_eq!(
                    tokio::time::timeout(Duration::from_secs(10), starts.recv())
                        .await
                        .unwrap(),
                    Some(0)
                );
                let current: Value =
                    serde_json::from_str(&conn.get::<_, String>(&key).await.unwrap()).unwrap();
                assert_eq!(current["status"], "processing");
                assert_ne!(current["updated_at"], old["updated_at"]);
                let first_claim: String = conn.get(format!("claim:{id}")).await.unwrap();
                first.requeue(&60).await.unwrap();
                assert_eq!(
                    conn.get::<_, String>(format!("claim:{id}")).await.unwrap(),
                    first_claim,
                    "a newly claimed old job is not stale"
                );
                let mut stale = current;
                stale["updated_at"] = old["updated_at"].clone();
                conn.set::<_, _, ()>(&key, stale.to_string()).await.unwrap();
                first.requeue(&60).await.unwrap();
                let running = second.clone();
                second_run = Some(tokio::spawn(async move {
                    running.run(vec![WorkerTarget::Shared.tag()]).await
                }));
                let second_claim = tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        if let Some(claim) = conn
                            .get::<_, Option<String>>(format!("claim:{id}"))
                            .await
                            .unwrap()
                            && claim != first_claim
                        {
                            break claim;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
                .await
                .unwrap();
                first.shutdown().unwrap();
                release[0].add_permits(1);
                // The new attempt may wait for the real provider semaphore until the old call returns.
                assert_eq!(
                    tokio::time::timeout(Duration::from_secs(10), starts.recv())
                        .await
                        .unwrap(),
                    Some(1)
                );
                tokio::time::timeout(Duration::from_secs(10), async {
                    while !first_run.is_finished() {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
                .await
                .unwrap();
                let still_running: Value =
                    serde_json::from_str(&conn.get::<_, String>(&key).await.unwrap()).unwrap();
                assert_eq!(
                    still_running["status"], "processing",
                    "old completion/failure must not overwrite the new attempt"
                );
                assert_eq!(
                    conn.get::<_, String>(format!("claim:{id}")).await.unwrap(),
                    second_claim
                );
                second.shutdown().unwrap();
                release[1].add_permits(1);
            })
            .catch_unwind()
            .await;
            release[0].add_permits(1);
            release[1].add_permits(1);
            yorishiro::App::on_shutdown(&worker_ctx).await;
            first.shutdown().unwrap();
            second.shutdown().unwrap();
            let drained_first = tokio::time::timeout(Duration::from_secs(10), first_run).await;
            let drained_second = if let Some(worker) = second_run {
                Some(tokio::time::timeout(Duration::from_secs(10), worker).await)
            } else {
                None
            };
            first.close().await.unwrap();
            second.close().await.unwrap();
            if let Err(panic) = outcome {
                std::panic::resume_unwind(panic);
            }
            drained_first.unwrap().unwrap().unwrap();
            drained_second.unwrap().unwrap().unwrap().unwrap();
            let completed: Value =
                serde_json::from_str(&conn.get::<_, String>(&key).await.unwrap()).unwrap();
            assert_eq!(completed["status"], "completed");
        })
        .await;
    }
}
