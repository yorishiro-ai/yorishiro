//! The production adapter that enqueues infer-fill jobs, beside the community dispatcher it shares a type with.

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;

use crate::edition::ee::workers::infer_fill::{
    InferFillArgs, InferFillDispatcher, InferFillWorker,
};
use crate::workers::dispatch::{JobSpec, LocoJobDispatcher, dispatch_job};
use crate::workers::embedding_sync::WorkerClass;

#[async_trait]
impl InferFillDispatcher for LocoJobDispatcher {
    async fn dispatch(&self, ctx: &AppContext, args: InferFillArgs) -> loco_rs::Result<String> {
        let spec = JobSpec {
            job_name: InferFillArgs::JOB_NAME,
            workspace_id: args.workspace_id,
            class: WorkerClass::Shared,
            dispatch_payload: serde_json::to_value(&args)
                .map_err(|error| loco_rs::Error::Message(error.to_string()))?,
            worker_name: InferFillWorker::class_name(),
            use_queue_concurrency: true,
        };
        dispatch_job(ctx, spec, |lifecycle_id, priority, target| {
            let args = InferFillArgs {
                lifecycle_id: Some(lifecycle_id),
                ..args
            };
            async move {
                crate::workers::target::enqueue::<InferFillWorker, _>(
                    ctx,
                    target,
                    args,
                    Some(priority),
                )
                .await
            }
        })
        .await
    }
}
