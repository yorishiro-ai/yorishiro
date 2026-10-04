use loco_rs::bgworker::BackgroundWorker;
use yorishiro::workers::reindex::{
    ReindexWorkerOfficial, ReindexWorkerShared, ReindexWorkerTenantPrivate,
};

#[test]
fn each_reindex_worker_type_carries_only_its_class_tag() {
    assert_eq!(
        ReindexWorkerTenantPrivate::tags(),
        vec!["worker-class:tenant-private".to_string()]
    );
    assert_eq!(
        ReindexWorkerOfficial::tags(),
        vec!["worker-class:official".to_string()]
    );
    assert_eq!(
        ReindexWorkerShared::tags(),
        vec!["worker-class:shared".to_string()]
    );
}

#[test]
fn reindex_worker_types_have_distinct_queue_class_names() {
    let names = [
        ReindexWorkerTenantPrivate::class_name(),
        ReindexWorkerOfficial::class_name(),
        ReindexWorkerShared::class_name(),
    ];
    assert_eq!(
        names.iter().collect::<std::collections::HashSet<_>>().len(),
        names.len()
    );
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
        async fn dispatch(&self, _ctx: &AppContext, args: ReindexArgs) -> loco_rs::Result<String> {
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
            workspace_id: Uuid::now_v7(),
            worker_class: WorkerClass::TenantPrivate,
        };
        let dispatcher = RecordingDispatcher {
            args: Mutex::new(Vec::new()),
            fail: false,
        };

        enqueue_for_class_with_dispatcher(&ctx, args.clone(), &dispatcher)
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
                workspace_id: Uuid::now_v7(),
                worker_class: WorkerClass::Shared,
            },
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

        let error = enqueue_reindex_with_dispatcher(&ctx, Uuid::now_v7(), &dispatcher)
            .await
            .expect_err("missing queue must fail");

        assert_eq!(error.to_string(), "no queue provider configured");
        assert!(dispatcher.args.lock().unwrap().is_empty());
    }
}
