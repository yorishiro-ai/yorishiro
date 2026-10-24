//! The infer-fill enqueue adapter.

use loco_rs::{app::AppContext, bgworker::BackgroundWorker};

use crate::edition::ee::workers::infer_fill::{InferFillArgs, InferFillWorker};
use crate::workers::dispatch::{JobSpec, dispatch_job};
use crate::workers::embedding_sync::WorkerClass;

pub(crate) async fn dispatch_infer_fill(
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
        worker_name: InferFillWorker::class_name(),
        use_queue_concurrency: true,
    };
    dispatch_job(ctx, spec, txn).await
}
