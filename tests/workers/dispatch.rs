use uuid::Uuid;
use yorishiro::models::queue_job_lifecycles::{Enqueue, Entity, LifecycleStatus};
use yorishiro::workers::embedding_sync::{EmbeddingSyncArgs, EmbeddingSyncWorker, WorkerClass};
use yorishiro::workers::target::WorkerTarget;

/// What a provider swap must not change: a target keeps draining its own jobs however many another target has queued.
///
/// Redis filters tags client-side over the first 1000 entries of a queue, where the SQL providers filter in the query, so every target has to get a queue of its own on Redis.
/// Runs only where the lane's queue is the reserved Valkey database.
mod redis_routing {
    use std::sync::Arc;
    use std::time::Duration;

    use loco_rs::bgworker::{self, BackgroundWorker};
    use loco_rs::config::RedisQueueConfig;
    use sea_orm::EntityTrait;

    use super::*;

    fn args(class: WorkerClass, lifecycle_id: Option<Uuid>) -> EmbeddingSyncArgs {
        EmbeddingSyncArgs {
            lifecycle_id,
            workspace_id: Uuid::nil(),
            entity_id: Uuid::nil(),
            worker_class: class,
        }
    }

    #[tokio::test]
    #[serial_test::serial(queue_postgres)]
    #[serial_test::serial(process_environment)]
    async fn a_target_drains_its_own_jobs_behind_a_backlog_of_another_target() {
        let Some(uri) = crate::valkey_test_url() else {
            eprintln!(
                "skipping Valkey routing test: the lane's queue is not the reserved Valkey database"
            );
            return;
        };
        crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
            // A worker process polls the queue of each target it serves, which `App::boot` configures from its tags.
            let queue = Arc::new(
                bgworker::redis::create_provider(&RedisQueueConfig {
                    uri,
                    dangerously_flush: false,
                    queues: Some(vec![
                        WorkerTarget::Shared.queue(),
                        WorkerTarget::Official.queue(),
                    ]),
                    num_workers: 1,
                    reaper: None,
                })
                .await
                .expect("the worker's Redis queue provider"),
            );
            queue.setup().await.expect("set up Redis queue");
            queue.clear().await.expect("clear Redis queue");
            queue
                .register(EmbeddingSyncWorker::build(&ctx))
                .await
                .expect("register the embedding worker");

            // More than the 1000 entries Redis scans for a tag match.
            queue
                .enqueue_batch(
                    EmbeddingSyncWorker::class_name(),
                    Some(WorkerTarget::Official.queue()),
                    (0..1100)
                        .map(|_| (args(WorkerClass::Official, None), Some(200)))
                        .collect(),
                    Some(vec![WorkerTarget::Official.tag()]),
                )
                .await
                .expect("enqueue the official backlog");

            let id = Uuid::now_v7();
            Entity::record_enqueue(
                &ctx.db,
                Enqueue {
                    id,
                    job_name: "redis-routing-test",
                    worker_class: WorkerClass::Shared,
                    workspace_id: None,
                    plan: None,
                    concurrency_key: None,
                    concurrency_limit: None,
                },
            )
            .await
            .unwrap();
            queue
                .enqueue(
                    EmbeddingSyncWorker::class_name(),
                    Some(WorkerTarget::Shared.queue()),
                    args(WorkerClass::Shared, Some(id)),
                    Some(vec![WorkerTarget::Shared.tag()]),
                    Some(100),
                )
                .await
                .expect("enqueue the shared job");

            let running = queue.clone();
            let handle =
                tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
            let completed = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let row = Entity::find_by_id(id).one(&ctx.db).await.unwrap().unwrap();
                    if row.status == LifecycleStatus::Completed.as_db_str() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await;
            let _ = queue.shutdown();
            let _ = handle.await;
            let _ = queue.clear().await;

            assert!(
                completed.is_ok(),
                "the shared job was never run behind the official backlog"
            );
        })
        .await;
    }
}
