//! A target drains independently of a backlog on another target's named queue.

use std::time::Duration;

use futures::FutureExt;
use loco_rs::app::Hooks;
use loco_rs::bgworker::{BackgroundWorker, Job, JobStatus};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::workers::query_embedding::{QueryEmbeddingArgs, QueryEmbeddingWorker};
use yorishiro::workers::target::WorkerTarget;

#[tokio::test]
async fn a_target_drains_its_own_jobs_behind_a_backlog_of_another_target() {
    crate::requests::boot_request_isolated::<App, _, _>(|_request, ctx| async move {
        let loco_rs::config::QueueConfig::Redis(config) = ctx.config.queue.as_ref().unwrap() else {
            return;
        };
        let producer = ctx.queue_provider.as_ref().unwrap();
        // Late deliveries have no pending request, so production query workers acknowledge them without an external provider call.
        let payload = serde_json::to_value(QueryEmbeddingArgs {
            request_id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
        })
        .unwrap();
        let backlog = producer
            .enqueue_batch(
                QueryEmbeddingWorker::class_name(),
                Some(WorkerTarget::Official.queue()),
                (0..1100).map(|_| (payload.clone(), Some(200))).collect(),
                Some(vec![WorkerTarget::Official.tag()]),
            )
            .await
            .unwrap();
        assert_eq!(backlog.len(), 1100);
        let shared = producer
            .enqueue(
                QueryEmbeddingWorker::class_name(),
                Some(WorkerTarget::Shared.queue()),
                payload,
                Some(vec![WorkerTarget::Shared.tag()]),
                Some(100),
            )
            .await
            .unwrap()
            .unwrap();
        let client = redis::Client::open(config.uri.clone()).unwrap();
        let mut observer = client.get_multiplexed_async_connection().await.unwrap();
        let keys: Vec<_> = backlog
            .iter()
            .chain(std::iter::once(&shared))
            .map(|id| format!("job:{id}"))
            .collect();
        {
            let target = WorkerTarget::Shared;
            let queue = crate::requests::query_worker::target_queue(&ctx, target).await;
            let mut worker_ctx = ctx.clone();
            worker_ctx.queue_provider = Some(queue.clone());
            App::connect_workers(&worker_ctx, &queue).await.unwrap();
            let running = queue.clone();
            let worker = tokio::spawn(async move { running.run(vec![target.tag()]).await });
            let outcome = std::panic::AssertUnwindSafe(async {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                loop {
                    // Observe only this case's deliveries, not every retained job in the shared deployment.
                    let records: Vec<String> = redis::cmd("MGET")
                        .arg(&keys)
                        .query_async(&mut observer)
                        .await
                        .unwrap();
                    let jobs: Vec<Job> = records
                        .iter()
                        .map(|record| serde_json::from_str(record).unwrap())
                        .collect();
                    let shared_job = jobs.iter().find(|job| job.id == shared).unwrap();
                    assert_ne!(shared_job.status, JobStatus::Failed, "{}", shared_job.data);
                    if shared_job.status == JobStatus::Completed {
                        assert!(backlog.iter().all(|id| {
                            jobs.iter()
                                .any(|job| job.id == *id && job.status == JobStatus::Queued)
                        }));
                        break;
                    }
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "the shared target completes independently of the official backlog"
                    );
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .catch_unwind()
            .await;
            App::on_shutdown(&worker_ctx).await;
            queue.shutdown().unwrap();
            let drained = tokio::time::timeout(Duration::from_secs(10), worker).await;
            queue.close().await.unwrap();
            match outcome {
                Ok(()) => {}
                Err(panic) => std::panic::resume_unwind(panic),
            }
            drained.unwrap().unwrap().unwrap();
        }
    })
    .await;
}
