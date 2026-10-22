use crate::requests::fixtures::{TenantArgs, TestTenant};
use crate::requests::{boot_request, boot_request_isolated};
use sea_orm::{ActiveModelTrait, ConnectionTrait, Set, TransactionTrait};
use serde_json::json;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::edition::ee::models::inference_jobs::{self, InferenceJobStatus};
use yorishiro::models::_entities::inference_jobs as generated;

#[test]
fn status_values_round_trip_through_db_and_json_strings() {
    for (status, wire) in [
        (InferenceJobStatus::Queued, "queued"),
        (InferenceJobStatus::Running, "running"),
        (InferenceJobStatus::Completed, "completed"),
        (InferenceJobStatus::Failed, "failed"),
    ] {
        assert_eq!(status.as_db_str(), wire);
        assert_eq!(InferenceJobStatus::from_db_str(wire), Some(status));
        assert_eq!(serde_json::to_value(status).unwrap(), json!(wire));
        assert_eq!(
            serde_json::from_value::<InferenceJobStatus>(json!(wire)).unwrap(),
            status
        );
    }
}

#[tokio::test]
async fn persisted_row_with_unknown_status_returns_internal_error() {
    if !crate::require_sqlite_backend() {
        return;
    }
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        assert_eq!(InferenceJobStatus::from_db_str("paused"), None);
        let fixture = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        // Corrupt-row injection must pin PRAGMA and writes to the same pooled connection.
        let db = ctx.db.begin().await.expect("begin corrupt-row fixture");
        db.execute_unprepared("PRAGMA ignore_check_constraints = ON")
            .await
            .expect("disable SQLite check constraints for corrupt-row fixture");

        let id = Uuid::new_v4();
        inference_jobs::create(&db, id, fixture.workspace_id, "notes")
            .await
            .expect("create job");
        let row = generated::ActiveModel {
            id: Set(id),
            status: Set("paused".into()),
            ..Default::default()
        }
        .update(&db)
        .await
        .expect("corrupt persisted status");
        let error = inference_jobs::get(&db, row.id).await.unwrap_err();
        assert!(matches!(error, yorishiro::YorishiroError::Internal(_)));
        let error = inference_jobs::InferenceJobRecord::try_from(row).unwrap_err();
        assert!(matches!(error, yorishiro::YorishiroError::Internal(_)));
        db.rollback().await.expect("discard corrupt-row fixture");
    })
    .await;
}

#[tokio::test]
async fn lifecycle_transitions_from_queued_to_completed_or_failed() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let fixture = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let db = ctx.db.clone();

        for terminal in [InferenceJobStatus::Completed, InferenceJobStatus::Failed] {
            let job_id = Uuid::new_v4();
            inference_jobs::create(&db, job_id, fixture.workspace_id, "notes")
                .await
                .expect("create job");
            assert_eq!(
                inference_jobs::get(&db, job_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                InferenceJobStatus::Queued
            );
            match terminal {
                InferenceJobStatus::Completed => {
                    assert!(inference_jobs::claim(&db, job_id).await.unwrap());
                    assert_eq!(
                        inference_jobs::get(&db, job_id)
                            .await
                            .unwrap()
                            .unwrap()
                            .status,
                        InferenceJobStatus::Running
                    );
                    inference_jobs::complete(&db, job_id, 3, 1).await.unwrap()
                }
                InferenceJobStatus::Failed => {
                    assert!(inference_jobs::claim(&db, job_id).await.unwrap());
                    assert_eq!(
                        inference_jobs::get(&db, job_id)
                            .await
                            .unwrap()
                            .unwrap()
                            .status,
                        InferenceJobStatus::Running
                    );
                    inference_jobs::fail(&db, job_id, "provider failed")
                        .await
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let record = inference_jobs::get(&db, job_id).await.unwrap().unwrap();
            assert_eq!(record.status, terminal);
        }
    })
    .await;
}

#[tokio::test]
async fn claim_rejects_stale_and_invalid_rows() {
    if !crate::require_sqlite_backend() {
        return;
    }
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let fixture = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        // Corrupt-row injection must pin PRAGMA and writes to the same pooled connection.
        let db = ctx.db.begin().await.expect("begin corrupt-row fixture");

        let job_id = Uuid::new_v4();
        inference_jobs::create(&db, job_id, fixture.workspace_id, "notes")
            .await
            .expect("create job");
        assert!(inference_jobs::claim(&db, job_id).await.unwrap());
        assert!(!inference_jobs::claim(&db, job_id).await.unwrap());
        inference_jobs::complete(&db, job_id, 1, 0).await.unwrap();
        assert!(!inference_jobs::claim(&db, job_id).await.unwrap());

        let missing = inference_jobs::claim(&db, Uuid::new_v4()).await.unwrap();
        assert!(!missing);
        assert!(inference_jobs::get(&db, job_id).await.unwrap().is_some());

        db.execute_unprepared("PRAGMA ignore_check_constraints = ON")
            .await
            .expect("disable SQLite check constraints for corrupt-row fixture");
        let invalid_id = Uuid::new_v4();
        inference_jobs::create(&db, invalid_id, fixture.workspace_id, "notes")
            .await
            .expect("create job to corrupt");
        generated::ActiveModel {
            id: Set(invalid_id),
            status: Set("paused".into()),
            ..Default::default()
        }
        .update(&db)
        .await
        .expect("corrupt claim status");
        assert!(matches!(
            inference_jobs::claim(&db, invalid_id).await,
            Err(yorishiro::YorishiroError::Internal(_))
        ));
        db.rollback().await.expect("discard corrupt-row fixture");
    })
    .await;
}
