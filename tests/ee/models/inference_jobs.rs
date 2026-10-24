use uuid::Uuid;
use yorishiro::edition::ee::models::inference_jobs::*;

#[tokio::test]
async fn reconcile_recovers_after_failure_before_retry() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let db = ctx.db.clone();
        let id = Uuid::now_v7();
        create(&db, id, fixture.workspace_id, "notes")
            .await
            .unwrap();
        assert!(claim(&db, id).await.unwrap());
        let attempt = get(&db, id).await.unwrap().unwrap().attempt;
        assert!(
            fail_attempt(&db, id, attempt, "provider failed")
                .await
                .unwrap()
        );
        assert!(reconcile_attempt(&db, id, attempt).await.unwrap());
        assert!(!fail_attempt(&db, id, attempt - 1, "stale").await.unwrap());
        assert!(
            complete_proposals_attempt(&db, id, attempt, 2, 0)
                .await
                .unwrap()
        );
        assert!(
            !complete_proposals_attempt(&db, id, attempt, 9, 0)
                .await
                .unwrap()
        );
        assert_eq!(
            get(&db, id).await.unwrap().unwrap().status,
            InferenceJobStatus::Completed
        );
    })
    .await;
}
