use async_trait::async_trait;
use loco_rs::prelude::*;
use std::sync::Mutex;
use uuid::Uuid;

use yorishiro::ee::workers::infer_fill::*;

struct RecordingDispatcher {
    args: Mutex<Vec<InferFillArgs>>,
    fail: bool,
}

#[async_trait]
impl InferFillDispatcher for RecordingDispatcher {
    async fn dispatch(&self, _ctx: &AppContext, args: InferFillArgs) -> loco_rs::Result<String> {
        self.args.lock().unwrap().push(args);
        if self.fail {
            Err(loco_rs::Error::Message("dispatch failed".into()))
        } else {
            Ok("infer-fill-job".into())
        }
    }
}

#[tokio::test]
async fn dispatches_the_original_args_and_returns_success() {
    let ctx = crate::workers::test_context().await;
    let args = InferFillArgs {
        lifecycle_id: None,
        job_id: Uuid::now_v7(),
        workspace_id: Uuid::now_v7(),
        schema_name: "note".into(),
    };
    let dispatcher = RecordingDispatcher {
        args: Mutex::new(Vec::new()),
        fail: false,
    };

    let job_id = enqueue_infer_fill_with_dispatcher(&ctx, args.clone(), &dispatcher)
        .await
        .expect("dispatch");

    assert_eq!(job_id, "infer-fill-job");
    let recorded = dispatcher.args.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].job_id, args.job_id);
    assert_eq!(recorded[0].workspace_id, args.workspace_id);
    assert_eq!(recorded[0].schema_name, args.schema_name);
}

#[tokio::test]
async fn preserves_dispatch_failure() {
    let ctx = crate::workers::test_context().await;
    let dispatcher = RecordingDispatcher {
        args: Mutex::new(Vec::new()),
        fail: true,
    };

    let error = enqueue_infer_fill_with_dispatcher(
        &ctx,
        InferFillArgs {
            lifecycle_id: None,
            job_id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
            schema_name: "note".into(),
        },
        &dispatcher,
    )
    .await
    .expect_err("dispatch must fail");

    assert_eq!(error.to_string(), "dispatch failed");
}

#[tokio::test]
async fn rejects_a_missing_queue_before_creating_a_job() {
    let ctx = crate::workers::test_context().await;

    let error = enqueue_infer_fill(&ctx, Uuid::now_v7(), "note".into())
        .await
        .expect_err("missing queue must fail");

    assert_eq!(
        error.to_string(),
        "infer-fill requires a queue provider (configure queue: in the server config)"
    );
}
