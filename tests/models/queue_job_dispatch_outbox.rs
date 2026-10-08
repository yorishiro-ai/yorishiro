use migration::{Migrator, MigratorTrait};
use sea_orm::Database;
use uuid::Uuid;
use yorishiro::models::queue_job_dispatch_outbox::{claim_due, due, record};
use yorishiro::models::queue_job_lifecycles::Enqueue;
use yorishiro::workers::embedding_sync::WorkerClass;

#[tokio::test]
async fn record_is_recoverable_until_attempted() {
    let path = tempfile::tempdir().unwrap().keep().join("outbox.sqlite3");
    let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    Migrator::up(&db, None).await.unwrap();
    let id = Uuid::now_v7();
    record(
        &db,
        Enqueue {
            id,
            job_name: "test",
            worker_class: WorkerClass::Shared,
            workspace_id: None,
            plan: None,
            concurrency_key: Some("shared"),
            concurrency_limit: Some(1),
        },
        serde_json::json!({"value": 7}),
        "TestWorker".into(),
        None,
        Some(vec!["test-tag".into()]),
    )
    .await
    .unwrap();

    let rows = due(&db).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].lifecycle_id, id);
    assert_eq!(rows[0].payload["lifecycle_id"], id.to_string());
    assert_eq!(rows[0].worker_name, "TestWorker");
    assert_eq!(rows[0].tags, Some(vec!["test-tag".into()]));

    assert!(claim_due(&db, id).await.unwrap());
    assert!(!claim_due(&db, id).await.unwrap());
    assert!(due(&db).await.unwrap().is_empty());
}
