use yorishiro::workers::embedding_sync::WorkerClass;

use yorishiro::workers::queue::*;

#[tokio::test]
async fn the_installed_policy_decides_and_a_missing_one_refuses() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let mut expected = Vec::new();
        for class in [
            WorkerClass::TenantPrivate,
            WorkerClass::Official,
            WorkerClass::Shared,
        ] {
            expected.push((
                class,
                concurrency_for(&ctx, fixture.workspace_id, class)
                    .await
                    .unwrap(),
            ));
        }
        let installed = ctx
            .shared_store
            .remove::<std::sync::Arc<dyn QueuePolicy>>()
            .expect("boot installs the edition's production policy");

        let refused = concurrency_for(&ctx, fixture.workspace_id, WorkerClass::Shared).await;
        assert!(refused.is_err_and(|error| error.contains("no policy is installed")));

        ctx.shared_store.insert(installed);
        for (class, expected) in expected {
            let policy = concurrency_for(&ctx, fixture.workspace_id, class)
                .await
                .unwrap();
            assert_eq!((policy.plan, policy.limit), (expected.plan, expected.limit));
        }
    })
    .await;
}

#[test]
fn each_class_is_admitted_at_its_own_band_without_borrowing_another_classs_capacity() {
    assert!(priority(WorkerClass::TenantPrivate) > priority(WorkerClass::Official));
    assert!(priority(WorkerClass::Official) > priority(WorkerClass::Shared));
}

#[cfg(feature = "enterprise")]
#[tokio::test]
async fn unlicensed_policy_bypasses_plan_and_workspace_resolution() {
    use std::sync::Arc;
    use yorishiro::edition::ee::controllers::middleware::edition::LicenceState;

    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let _ = ctx.shared_store.remove::<Arc<LicenceState>>();
        for class in [
            WorkerClass::Shared,
            WorkerClass::Official,
            WorkerClass::TenantPrivate,
        ] {
            let policy = concurrency_for(&ctx, uuid::Uuid::new_v4(), class)
                .await
                .unwrap();
            assert_eq!(policy.plan, "community");
            assert_eq!(policy.limit, None);
        }
    })
    .await;
}
