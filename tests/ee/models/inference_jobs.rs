use migration::{Migrator, MigratorTrait};
use sea_orm::Database;
use sea_orm::{ActiveModelTrait, Set};
use uuid::Uuid;
use yorishiro::ee::models::inference_jobs::*;

#[tokio::test]
async fn reconcile_recovers_after_failure_before_retry() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    let tenant = yorishiro::models::_entities::tenant_tenants::ActiveModel {
        name: Set("reconcile-tenant".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    let workspace = yorishiro::models::_entities::workspace_workspaces::ActiveModel {
        tenant_id: Set(tenant.id),
        name: Set("reconcile-workspace".into()),
        status: Set("active".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    let id = Uuid::now_v7();
    create(&db, id, workspace.id, "notes").await.unwrap();
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
}
