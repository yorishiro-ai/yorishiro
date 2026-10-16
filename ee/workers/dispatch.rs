//! The production adapter that enqueues infer-fill jobs, beside the community dispatcher it shares a type with.

use async_trait::async_trait;
use loco_rs::app::AppContext;

use crate::edition::ee::workers::infer_fill::{InferFillArgs, InferFillDispatcher};
use crate::workers::dispatch::{JobSpec, LocoJobDispatcher, dispatch_job};
use crate::workers::embedding_sync::WorkerClass;

#[async_trait]
impl InferFillDispatcher for LocoJobDispatcher {
    async fn dispatch(
        &self,
        ctx: &AppContext,
        args: InferFillArgs,
        txn: Option<&sea_orm::DatabaseTransaction>,
    ) -> loco_rs::Result<String> {
        let spec = JobSpec {
            job_name: InferFillArgs::JOB_NAME,
            workspace_id: args.workspace_id,
            class: WorkerClass::Shared,
            dispatch_payload: serde_json::to_value(&args)
                .map_err(|error| loco_rs::Error::Message(error.to_string()))?,
            worker_name: "InferFillWorker".to_owned(),
            use_queue_concurrency: true,
        };
        dispatch_job(ctx, spec, txn).await
    }
}
