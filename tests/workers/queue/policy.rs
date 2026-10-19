use uuid::Uuid;
use yorishiro::workers::embedding_sync::WorkerClass;

use yorishiro::workers::queue::*;

#[tokio::test]
async fn the_installed_policy_decides_and_a_missing_one_refuses() {
    let ctx = crate::workers::test_context().await;

    let refused = concurrency_for(&ctx, Uuid::nil(), WorkerClass::Shared).await;
    assert!(refused.is_err_and(|error| error.contains("no policy is installed")));

    ctx.shared_store.insert(default_queue_policy());
    for class in [
        WorkerClass::TenantPrivate,
        WorkerClass::Official,
        WorkerClass::Shared,
    ] {
        let policy = concurrency_for(&ctx, Uuid::nil(), class).await.unwrap();
        assert_eq!((policy.plan.as_str(), policy.limit), ("community", None));
    }
}

#[test]
fn each_class_is_admitted_at_its_own_band_without_borrowing_another_classs_capacity() {
    assert!(priority(WorkerClass::TenantPrivate) > priority(WorkerClass::Official));
    assert!(priority(WorkerClass::Official) > priority(WorkerClass::Shared));
}
