use loco_rs::bgworker::BackgroundWorker;
use yorishiro::workers::reindex::ReindexWorker;

/// The worker declares neither tags nor a queue: a job's target is chosen when the job is enqueued.
#[test]
fn the_reindex_worker_declares_no_routing_of_its_own() {
    assert!(ReindexWorker::tags().is_empty());
    assert!(ReindexWorker::queue().is_none());
}

/// Loco persists the class name, so renaming the worker strands queued jobs.
#[test]
fn the_reindex_worker_keeps_its_queue_class_name() {
    assert_eq!(ReindexWorker::class_name(), "ReindexWorker");
}

#[tokio::test]
async fn rejects_a_missing_queue_before_dispatch() {
    use uuid::Uuid;
    use yorishiro::workers::reindex::enqueue_reindex;

    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, mut ctx| async move {
        ctx.queue_provider = None;
        let error = enqueue_reindex(&ctx, Uuid::now_v7(), None)
            .await
            .expect_err("missing queue must fail");
        assert_eq!(error.to_string(), "no queue provider configured");
    })
    .await;
}
