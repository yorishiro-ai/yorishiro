//! The production adapter that enqueues infer-fill jobs, beside the community dispatcher it shares a type with.

use async_trait::async_trait;
use loco_rs::app::AppContext;

use crate::edition::ee::workers::infer_fill::{
    InferFillArgs, InferFillDispatcher, InferFillWorker,
};
use crate::workers::dispatch::{JobSpec, LocoJobDispatcher, dispatch_job, worker_route};
use crate::workers::embedding_sync::WorkerClass;

#[async_trait]
impl InferFillDispatcher for LocoJobDispatcher {
    async fn dispatch(&self, ctx: &AppContext, args: InferFillArgs) -> loco_rs::Result<String> {
        use loco_rs::bgworker::BackgroundWorker;

        let route = worker_route::<InferFillWorker, _>();
        let spec = JobSpec {
            job_name: InferFillArgs::JOB_NAME,
            workspace_id: args.workspace_id,
            class: WorkerClass::Shared,
            dispatch_payload: serde_json::to_value(&args)
                .map_err(|error| loco_rs::Error::Message(error.to_string()))?,
            route,
            use_queue_concurrency: true,
        };
        dispatch_job(ctx, spec, |lifecycle_id, priority| {
            let args = InferFillArgs {
                lifecycle_id: Some(lifecycle_id),
                ..args
            };
            async move {
                InferFillWorker::perform_later_with_priority(ctx, args, Some(priority)).await
            }
        })
        .await
    }
}
