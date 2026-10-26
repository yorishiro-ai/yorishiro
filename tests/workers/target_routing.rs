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

#[tokio::test]
async fn production_dispatch_records_each_resolved_target() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use yorishiro::workers::embedding_sync::WorkerClass;
    use yorishiro::workers::reindex::{ReindexArgs, enqueue_for_class};

    crate::requests::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        for class in [
            WorkerClass::Shared,
            WorkerClass::Official,
            WorkerClass::TenantPrivate,
        ] {
            enqueue_for_class(
                &ctx,
                ReindexArgs {
                    lifecycle_id: None,
                    workspace_id: fixture.workspace_id,
                    worker_class: class,
                    startup: false,
                },
            )
            .await
            .unwrap();
            let lifecycle = yorishiro::models::queue_job_lifecycles::Entity::find()
                .filter(
                    yorishiro::models::queue_job_lifecycles::Column::WorkspaceId
                        .eq(fixture.workspace_id),
                )
                .filter(
                    yorishiro::models::queue_job_lifecycles::Column::WorkerClass
                        .eq(class.as_db_str()),
                )
                .filter(yorishiro::models::queue_job_lifecycles::Column::JobName.eq("reindex"))
                .one(&ctx.db)
                .await
                .unwrap()
                .expect("production dispatch records the lifecycle");
            let row = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(
                lifecycle.id,
            )
            .one(&ctx.db)
            .await
            .unwrap()
            .expect("production dispatch records the delivery");
            let expected = match class {
                WorkerClass::Shared => "worker-target:shared".to_owned(),
                WorkerClass::Official => "worker-target:official".to_owned(),
                WorkerClass::TenantPrivate => format!("worker-target:tenant:{}", fixture.tenant_id),
            };
            assert_eq!(row.queue_name.as_deref(), Some(expected.as_str()));
            assert_eq!(row.tags, Some(serde_json::json!([expected])));
            assert_eq!(
                row.payload["workspace_id"],
                fixture.workspace_id.to_string()
            );
        }
    })
    .await;
}

#[tokio::test]
async fn injected_deliveries_are_consumed_only_by_their_target() {
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
            let mut worker_ctx = ctx.clone();
            let queue = crate::requests::query_worker::target_queue(&mut worker_ctx, target).await;
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
