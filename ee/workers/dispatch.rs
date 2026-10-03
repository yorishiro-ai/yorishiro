//! The production adapter that enqueues infer-fill jobs, beside the community dispatcher it shares a type with.

use async_trait::async_trait;
use loco_rs::{app::AppContext, bgworker::BackgroundWorker};

use crate::ee::workers::infer_fill::{InferFillArgs, InferFillDispatcher, InferFillWorker};
use crate::workers::dispatch::LocoJobDispatcher;
use crate::workers::embedding_sync::WorkerClass;

#[async_trait]
impl InferFillDispatcher for LocoJobDispatcher {
    async fn dispatch(&self, ctx: &AppContext, mut args: InferFillArgs) -> loco_rs::Result<String> {
        use loco_rs::bgworker::BackgroundWorker;

        let lifecycle_id = uuid::Uuid::now_v7();
        let scheduling = crate::workers::queue::decide(WorkerClass::Shared);
        tracing::info!(
            lifecycle_id = %lifecycle_id,
            worker_class = "shared",
            scheduling_priority = scheduling.priority,
            fallback = scheduling.fallback,
            "queue scheduling decision"
        );
        crate::models::queue_job_lifecycles::Entity::record_enqueue(
            &ctx.db,
            crate::models::queue_job_lifecycles::Enqueue {
                id: lifecycle_id,
                job_name: "infer_fill",
                worker_class: "shared",
                workspace_id: Some(args.workspace_id),
                plan: None,
                concurrency_key: Some("shared"),
                concurrency_limit: Some(1),
            },
        )
        .await
        .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
        args.lifecycle_id = Some(lifecycle_id);
        let result =
            InferFillWorker::perform_later_with_priority(ctx, args, Some(scheduling.priority))
                .await;
        match result {
            Ok(job_id) => {
                if let Err(error) = crate::models::queue_job_lifecycles::Entity::mark_dispatched(
                    &ctx.db,
                    lifecycle_id,
                    &job_id,
                )
                .await
                {
                    tracing::error!(
                        lifecycle_id = %lifecycle_id,
                        provider_job_id = %job_id,
                        diagnostic = %error,
                        "provider job dispatched but lifecycle correlation write failed"
                    );
                }
                Ok(job_id)
            }
            Err(error) => {
                let _ = crate::models::queue_job_lifecycles::Entity::finish(
                    &ctx.db,
                    lifecycle_id,
                    None,
                    crate::models::queue_job_lifecycles::LifecycleStatus::Unavailable,
                    Some(&error.to_string()),
                )
                .await;
                Err(error)
            }
        }
    }
}
