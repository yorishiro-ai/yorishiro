//! Production workers consume only deliveries for the targets they serve.

use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use loco_rs::app::Hooks;
use loco_rs::bgworker::{BackgroundWorker, Job, JobStatus};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::workers::query_embedding::{QueryEmbeddingArgs, QueryEmbeddingWorker};
use yorishiro::workers::target::WorkerTarget;

#[test]
fn the_tags_are_the_tags_a_dispatch_writes() {
    assert_eq!("worker-target:shared", WorkerTarget::Shared.tag());
    assert_eq!("worker-target:official", WorkerTarget::Official.tag());
    let tenant = format!("worker-target:tenant:{}", Uuid::now_v7());
    assert_eq!(tenant, WorkerTarget::parse_tag(&tenant).unwrap().tag());
}

#[tokio::test]
async fn the_configured_queue_delivers_each_target_only_to_its_worker() {
    crate::requests::boot_request_isolated::<App, _, _>(|_request, ctx| async move {
        let targets = [
            WorkerTarget::Shared,
            WorkerTarget::Official,
            WorkerTarget::parse_tag(&format!("worker-target:tenant:{}", Uuid::now_v7())).unwrap(),
            WorkerTarget::parse_tag(&format!("worker-target:tenant:{}", Uuid::now_v7())).unwrap(),
        ];
        let producer = ctx.queue_provider.as_ref().unwrap();
        let mut ids = Vec::new();
        // Inject late query deliveries whose request rows are gone, so the real worker completes without calling an external provider.
        for target in &targets {
            ids.push(
                producer
                    .enqueue(
                        QueryEmbeddingWorker::class_name(),
                        Some(target.queue()),
                        serde_json::to_value(QueryEmbeddingArgs {
                            request_id: Uuid::now_v7(),
                            workspace_id: Uuid::now_v7(),
                        })
                        .unwrap(),
                        Some(vec![target.tag()]),
                        None,
                    )
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        // An obsolete class tag must not be accepted as a current target tag.
        ids.push(
            producer
                .enqueue(
                    QueryEmbeddingWorker::class_name(),
                    Some(WorkerTarget::Shared.queue()),
                    serde_json::to_value(QueryEmbeddingArgs {
                        request_id: Uuid::now_v7(),
                        workspace_id: Uuid::now_v7(),
                    })
                    .unwrap(),
                    Some(vec!["worker-class:shared".into()]),
                    None,
                )
                .await
                .unwrap()
                .unwrap(),
        );
        let directory = tempfile::tempdir().unwrap();
        for (index, target) in targets.into_iter().enumerate() {
            let queue = crate::requests::query_worker::target_queue(&ctx, target).await;
            let mut worker_ctx = ctx.clone();
            worker_ctx.queue_provider = Some(queue.clone());
            App::connect_workers(&worker_ctx, &queue).await.unwrap();
            let running = Arc::clone(&queue);
            let worker = tokio::spawn(async move { running.run(vec![target.tag()]).await });
            let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
                Duration::from_secs(10),
                async {
                    loop {
                        let dump = queue.dump(directory.path(), None, None).await.unwrap();
                        let jobs: Vec<Job> =
                            serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                        if jobs
                            .iter()
                            .any(|job| job.id == ids[index] && job.status == JobStatus::Completed)
                        {
                            for id in ids.iter().skip(index + 1) {
                                assert!(jobs.iter().any(|job| {
                                    job.id == *id && job.status == JobStatus::Queued
                                }));
                            }
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                },
            ))
            .catch_unwind()
            .await;
            App::on_shutdown(&worker_ctx).await;
            queue.shutdown().unwrap();
            let drained = tokio::time::timeout(Duration::from_secs(10), worker).await;
            queue.close().await.unwrap();
            match outcome {
                Ok(result) => result.expect("the production worker completes only its target"),
                Err(panic) => std::panic::resume_unwind(panic),
            }
            drained.unwrap().unwrap().unwrap();
        }
    })
    .await;
}
