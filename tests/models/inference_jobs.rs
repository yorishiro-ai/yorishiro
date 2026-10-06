use migration::{Migrator, MigratorTrait};
use sea_orm::{ActiveModelTrait, ActiveValue, ConnectionTrait, Database, EntityTrait, Set};
use serde_json::json;
use uuid::Uuid;
use yorishiro::edition::ee::models::inference_jobs::{self, InferenceJobStatus};
use yorishiro::models::_entities::{
    inference_jobs as generated, tenant_tenants, workspace_workspaces,
};

async fn database() -> (sea_orm::DatabaseConnection, tempfile::TempDir) {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let path = directory.path().join("inference_jobs.sqlite3");
    let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .expect("connect database");
    Migrator::up(&db, None).await.expect("migrate database");
    (db, directory)
}

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
    assert_eq!(InferenceJobStatus::from_db_str("paused"), None);
    let (db, _directory) = database().await;
    db.execute_unprepared("PRAGMA ignore_check_constraints = ON")
        .await
        .expect("disable SQLite check constraints for corrupt-row fixture");
    let tenant = tenant_tenants::ActiveModel {
        name: Set("inference-invalid-tenant".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert tenant");
    let workspace = workspace_workspaces::ActiveModel {
        tenant_id: Set(tenant.id),
        name: Set("inference-invalid-workspace".into()),
        status: Set("active".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert workspace");
    let now = chrono::Utc::now().fixed_offset();
    let row = generated::Model {
        id: Uuid::new_v4(),
        workspace_id: workspace.id,
        schema_name: "notes".into(),
        status: "paused".into(),
        applied: 0,
        proposed: 0,
        skipped: 0,
        error: None,
        attempt: 0,
        created_at: now,
        updated_at: now,
    };
    generated::ActiveModel {
        id: Set(row.id),
        workspace_id: Set(row.workspace_id),
        schema_name: Set(row.schema_name.clone()),
        status: Set(row.status.clone()),
        applied: Set(row.applied),
        proposed: Set(row.proposed),
        skipped: Set(row.skipped),
        error: Set(row.error.clone()),
        attempt: Set(row.attempt),
        created_at: Set(row.created_at),
        updated_at: Set(row.updated_at),
    }
    .insert(&db)
    .await
    .expect("insert corrupt status fixture");
    let error = inference_jobs::get(&db, row.id).await.unwrap_err();
    assert!(matches!(error, yorishiro::YorishiroError::Internal(_)));
    let error = inference_jobs::InferenceJobRecord::try_from(row).unwrap_err();
    assert!(matches!(error, yorishiro::YorishiroError::Internal(_)));
}

#[tokio::test]
async fn lifecycle_transitions_from_queued_to_completed_or_failed() {
    let (db, _directory) = database().await;
    let tenant = tenant_tenants::ActiveModel {
        name: Set("inference-tenant".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert tenant");
    let workspace = workspace_workspaces::ActiveModel {
        tenant_id: Set(tenant.id),
        name: Set("inference-test".into()),
        status: Set("active".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert workspace");

    for terminal in [InferenceJobStatus::Completed, InferenceJobStatus::Failed] {
        let job_id = Uuid::new_v4();
        inference_jobs::create(&db, job_id, workspace.id, "notes")
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
}

#[tokio::test]
async fn claim_rejects_stale_and_invalid_rows() {
    let (db, _directory) = database().await;
    let tenant = tenant_tenants::ActiveModel {
        name: ActiveValue::Set("inference-stale-tenant".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert tenant");
    let workspace = workspace_workspaces::ActiveModel {
        tenant_id: ActiveValue::Set(tenant.id),
        name: ActiveValue::Set("inference-stale-test".into()),
        status: ActiveValue::Set("active".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert workspace");
    let job_id = Uuid::new_v4();
    inference_jobs::create(&db, job_id, workspace.id, "notes")
        .await
        .expect("create job");
    assert!(inference_jobs::claim(&db, job_id).await.unwrap());
    assert!(!inference_jobs::claim(&db, job_id).await.unwrap());
    inference_jobs::complete(&db, job_id, 1, 0).await.unwrap();
    assert!(!inference_jobs::claim(&db, job_id).await.unwrap());

    let missing = inference_jobs::claim(&db, Uuid::new_v4()).await.unwrap();
    assert!(!missing);
    assert!(
        generated::Entity::find_by_id(job_id)
            .one(&db)
            .await
            .unwrap()
            .is_some()
    );

    db.execute_unprepared("PRAGMA ignore_check_constraints = ON")
        .await
        .expect("disable SQLite check constraints for corrupt-row fixture");
    let invalid_id = Uuid::new_v4();
    generated::ActiveModel {
        id: Set(invalid_id),
        workspace_id: Set(workspace.id),
        schema_name: Set("notes".into()),
        status: Set("paused".into()),
        applied: Set(0),
        proposed: Set(0),
        skipped: Set(0),
        error: Set(None),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert invalid claim fixture");
    assert!(matches!(
        inference_jobs::claim(&db, invalid_id).await,
        Err(yorishiro::YorishiroError::Internal(_))
    ));
}
