use loco_rs::bgworker::BackgroundWorker;
use yorishiro::workers::reindex::ReindexWorker;

/// The worker declares neither tags nor a queue: a job's target is chosen when the job is enqueued.
#[test]
fn the_reindex_worker_declares_no_routing_of_its_own() {
    assert!(ReindexWorker::tags().is_empty());
    assert!(ReindexWorker::queue().is_none());
}

/// Loco stores the class name on every queued job and finds the handler by it, so renaming the worker strands whatever is already queued.
#[test]
fn the_reindex_worker_keeps_its_queue_class_name() {
    assert_eq!(ReindexWorker::class_name(), "ReindexWorker");
}

mod unit {
    use async_trait::async_trait;
    use loco_rs::app::AppContext;

    use std::sync::Mutex;
    use uuid::Uuid;

    use yorishiro::workers::dispatch::ReindexDispatcher;
    use yorishiro::workers::embedding_sync::WorkerClass;

    use yorishiro::workers::reindex::*;

    struct RecordingDispatcher {
        args: Mutex<Vec<ReindexArgs>>,
        fail: bool,
    }

    #[async_trait]
    impl ReindexDispatcher for RecordingDispatcher {
        async fn dispatch(
            &self,
            _ctx: &AppContext,
            args: ReindexArgs,
            _txn: Option<&sea_orm::DatabaseTransaction>,
        ) -> loco_rs::Result<String> {
            self.args.lock().unwrap().push(args);
            if self.fail {
                Err(loco_rs::Error::Message("dispatch failed".into()))
            } else {
                Ok("reindex-job".into())
            }
        }
    }

    #[tokio::test]
    async fn dispatches_the_original_args_and_returns_success() {
        let ctx = crate::workers::test_context().await;
        let args = ReindexArgs {
            lifecycle_id: None,
            dispatch_id: None,
            publishing_epoch: None,
            workspace_id: Uuid::now_v7(),
            worker_class: WorkerClass::TenantPrivate,
            startup: false,
        };
        let dispatcher = RecordingDispatcher {
            args: Mutex::new(Vec::new()),
            fail: false,
        };

        enqueue_for_class_with_dispatcher(&ctx, args.clone(), None, &dispatcher)
            .await
            .expect("dispatch");

        let recorded = dispatcher.args.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].workspace_id, args.workspace_id);
        assert_eq!(recorded[0].worker_class, args.worker_class);
    }

    #[tokio::test]
    async fn preserves_dispatch_failure() {
        let ctx = crate::workers::test_context().await;
        let dispatcher = RecordingDispatcher {
            args: Mutex::new(Vec::new()),
            fail: true,
        };

        let error = enqueue_for_class_with_dispatcher(
            &ctx,
            ReindexArgs {
                lifecycle_id: None,
                dispatch_id: None,
                publishing_epoch: None,
                workspace_id: Uuid::now_v7(),
                worker_class: WorkerClass::Shared,
                startup: false,
            },
            None,
            &dispatcher,
        )
        .await
        .expect_err("dispatch must fail");

        assert_eq!(error.to_string(), "dispatch failed");
    }

    #[tokio::test]
    async fn rejects_a_missing_queue_before_dispatch() {
        let ctx = crate::workers::test_context().await;
        let dispatcher = RecordingDispatcher {
            args: Mutex::new(Vec::new()),
            fail: false,
        };

        let error = enqueue_reindex_with_dispatcher(&ctx, Uuid::now_v7(), None, &dispatcher)
            .await
            .expect_err("missing queue must fail");

        assert_eq!(error.to_string(), "no queue provider configured");
        assert!(dispatcher.args.lock().unwrap().is_empty());
    }
}
