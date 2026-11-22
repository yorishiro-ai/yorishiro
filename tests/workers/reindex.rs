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
