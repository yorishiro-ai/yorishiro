use uuid::Uuid;
use yorishiro::models::queue_job_lifecycles::{Enqueue, Entity, LifecycleStatus};
use yorishiro::workers::embedding_sync::{
    EmbeddingSyncArgs, EmbeddingSyncWorkerShared, WorkerClass,
};
/// What a provider swap must not change: a worker class keeps draining its own jobs however many another class has queued.
///
/// Redis filters tags client-side over the first 1000 entries of a queue, where the SQL providers filter in the query, so the production worker types have to give Redis a queue per class.
/// Runs only where the lane's queue is the reserved Valkey database.
mod redis_routing {
    use std::time::Duration;

    use loco_rs::bgworker::BackgroundWorker;
    use sea_orm::EntityTrait;

    use super::*;

    use yorishiro::workers::embedding_sync::EmbeddingSyncWorkerOfficial;

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
    async fn a_class_drains_its_own_jobs_behind_a_backlog_of_another_class() {
        if crate::valkey_test_url().is_none() {
            eprintln!(
                "skipping Valkey routing test: the lane's queue is not the reserved Valkey database"
            );
            return;
        }
        crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
            let queue = ctx
                .queue_provider
                .clone()
                .expect("the app's Redis queue provider");
            queue.clear().await.expect("clear Redis queue");
            queue
                .register(EmbeddingSyncWorkerShared::build(&ctx))
                .await
                .expect("register shared worker");

            // More than the 1000 entries Redis scans for a tag match.
            EmbeddingSyncWorkerOfficial::perform_all_later_with_priority(
                &ctx,
                (0..1100)
                    .map(|_| (args(WorkerClass::Official, None), Some(200)))
                    .collect(),
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
            EmbeddingSyncWorkerShared::perform_later_with_priority(
                &ctx,
                args(WorkerClass::Shared, Some(id)),
                Some(100),
            )
            .await
            .expect("enqueue the shared job");

            let running = queue.clone();
            let handle = tokio::spawn(async move {
                running
                    .run(vec![WorkerClass::Shared.tag().to_owned()])
                    .await
            });
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
