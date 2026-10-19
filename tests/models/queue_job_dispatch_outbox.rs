use chrono::Utc;
use migration::{Migrator, MigratorTrait};
use sea_orm::{
    ActiveModelTrait, Database, EntityTrait, IntoActiveModel, PaginatorTrait, TransactionTrait,
};
use uuid::Uuid;
use yorishiro::models::queue_job_dispatch_outbox::{due, record};
use yorishiro::models::queue_job_lifecycles::{Enqueue, Entity as Lifecycles};
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
    assert_eq!(rows[0].lifecycle_id, Some(id));
    assert_eq!(rows[0].payload["lifecycle_id"], id.to_string());
    assert_eq!(rows[0].worker_name, "TestWorker");
    assert_eq!(rows[0].tags, Some(vec!["test-tag".into()]));
}

#[tokio::test]
async fn durable_claim_excludes_a_second_relay_and_requires_matching_ack_token() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::connect(format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("claim.sqlite3").display()
    ))
    .await
    .unwrap();
    Migrator::up(&db, None).await.unwrap();
    let id = Uuid::now_v7();
    record(
        &db,
        Enqueue {
            id,
            job_name: "claim",
            worker_class: WorkerClass::Shared,
            workspace_id: Some(Uuid::now_v7()),
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
        serde_json::json!({}),
        "TestWorker".into(),
        None,
        None,
    )
    .await
    .unwrap();
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    assert!(
        yorishiro::models::queue_job_dispatch_outbox::claim_dispatch(&db, id, first, 30)
            .await
            .unwrap()
    );
    assert!(
        !yorishiro::models::queue_job_dispatch_outbox::claim_dispatch(&db, id, second, 30)
            .await
            .unwrap()
    );
    assert!(
        !yorishiro::models::queue_job_dispatch_outbox::acknowledge_dispatch(
            &db,
            id,
            second,
            "wrong-provider"
        )
        .await
        .unwrap()
    );
    assert!(
        yorishiro::models::queue_job_dispatch_outbox::acknowledge_dispatch(
            &db,
            id,
            first,
            "provider-1"
        )
        .await
        .unwrap()
    );
    db.close().await.unwrap();
}

#[tokio::test]
async fn recovery_eligibility_uses_lifecycle_status_and_retry_at() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox-eligibility.sqlite3");
    let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    Migrator::up(&db, None).await.unwrap();

    let cases = [
        ("queued", None, None, true),
        ("retrying", None, None, true),
        ("retrying", None, Some(-30), true),
        ("retrying", None, Some(30), false),
        ("running", Some("provider"), Some(-30), false),
        ("failed", None, None, false),
        ("completed", None, None, false),
        ("cancelled", None, None, false),
        ("unavailable", None, None, false),
    ];
    let now = Utc::now().fixed_offset();
    let mut expected = Vec::new();

    let workspace_id = Uuid::now_v7();
    for (status, provider_job_id, retry_offset, eligible) in cases {
        {
            let retry_at = retry_offset.map(|offset| now + chrono::Duration::days(offset));
            let id = Uuid::now_v7();
            record(
                &db,
                Enqueue {
                    id,
                    job_name: "eligibility-test",
                    worker_class: WorkerClass::Shared,
                    workspace_id: Some(workspace_id),
                    plan: None,
                    concurrency_key: None,
                    concurrency_limit: None,
                },
                serde_json::json!({"value": status, "workspace_id": workspace_id}),
                "TestWorker".into(),
                Some("test-queue".into()),
                Some(vec!["test-tag".into()]),
            )
            .await
            .unwrap();

            let mut lifecycle = Lifecycles::find_by_id(id)
                .one(&db)
                .await
                .unwrap()
                .unwrap()
                .into_active_model();
            lifecycle.status = sea_orm::ActiveValue::Set(status.into());
            lifecycle.provider_job_id = sea_orm::ActiveValue::Set(
                provider_job_id.map(|provider_job_id| format!("{provider_job_id}-{id}")),
            );
            lifecycle.retry_at = sea_orm::ActiveValue::Set(retry_at);
            lifecycle.update(&db).await.unwrap();
            if eligible {
                expected.push(id);
            }
        }
    }

    let mut actual: Vec<_> = due(&db)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|dispatch| dispatch.lifecycle_id)
        .collect();
    expected.sort();
    actual.sort();
    let missing: Vec<_> = expected
        .iter()
        .filter(|id| !actual.contains(id))
        .copied()
        .collect();
    let extra: Vec<_> = actual
        .iter()
        .filter(|id| !expected.contains(id))
        .copied()
        .collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "missing={missing:?} extra={extra:?}"
    );

    db.close().await.unwrap();
}

#[tokio::test]
async fn ordinary_retry_eligibility_uses_retry_at_not_provider_job_id() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::connect(format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("ordinary-retry.sqlite3").display()
    ))
    .await
    .unwrap();
    Migrator::up(&db, None).await.unwrap();
    let now = Utc::now().fixed_offset();
    let workspace_id = Uuid::now_v7();
    let due_id = Uuid::now_v7();
    record(
        &db,
        Enqueue {
            id: due_id,
            job_name: "ordinary-retry",
            worker_class: WorkerClass::Shared,
            workspace_id: Some(workspace_id),
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
        serde_json::json!({}),
        "TestWorker".into(),
        None,
        None,
    )
    .await
    .unwrap();
    let mut lifecycle = Lifecycles::find_by_id(due_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    lifecycle.status = sea_orm::ActiveValue::Set("retrying".into());
    lifecycle.provider_job_id = sea_orm::ActiveValue::Set(None);
    let mut outbox =
        yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(due_id)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
    outbox.published_at = sea_orm::ActiveValue::Set(None);
    outbox.update(&db).await.unwrap();
    lifecycle.retry_at = sea_orm::ActiveValue::Set(Some(now - chrono::Duration::seconds(1)));
    lifecycle.update(&db).await.unwrap();
    let late_id = Uuid::now_v7();
    record(
        &db,
        Enqueue {
            id: late_id,
            job_name: "ordinary-retry-late",
            worker_class: WorkerClass::Shared,
            workspace_id: None,
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
        serde_json::json!({}),
        "TestWorker".into(),
        None,
        None,
    )
    .await
    .unwrap();
    let mut late = Lifecycles::find_by_id(late_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    late.status = sea_orm::ActiveValue::Set("retrying".into());
    late.retry_at = sea_orm::ActiveValue::Set(Some(now + chrono::Duration::seconds(60)));
    late.update(&db).await.unwrap();
    let mut late_outbox =
        yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(late_id)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
    late_outbox.published_at = sea_orm::ActiveValue::Set(Some(now));
    late_outbox.update(&db).await.unwrap();
    let due_ids: Vec<_> = due(&db)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.lifecycle_id.unwrap())
        .collect();
    assert_eq!(due_ids, vec![due_id]);
    db.close().await.unwrap();
}

#[tokio::test]
async fn requeue_updates_the_existing_outbox_and_clears_provider_ack() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::connect(format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("requeue.sqlite3").display()
    ))
    .await
    .unwrap();
    Migrator::up(&db, None).await.unwrap();
    let id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
    record(
        &db,
        Enqueue {
            id,
            job_name: "retry",
            worker_class: WorkerClass::Shared,
            workspace_id: Some(workspace_id),
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
        serde_json::json!({"workspace_id": workspace_id.to_string(), "attempt": 1}),
        "TestWorker".into(),
        None,
        None,
    )
    .await
    .unwrap();
    yorishiro::models::queue_job_lifecycles::Entity::defer(&db, id, None, "retry")
        .await
        .unwrap();
    let mut lifecycle = Lifecycles::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    lifecycle.attempt = sea_orm::ActiveValue::Set(3);
    lifecycle.update(&db).await.unwrap();
    let txn = db.begin().await.unwrap();
    yorishiro::models::queue_job_dispatch_outbox::record_lifecycle_payload(
        &txn,
        id,
        serde_json::json!({
            "workspace_id": workspace_id.to_string(),
            "attempt": 2
        }),
        "TestWorker".into(),
        None,
        None,
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    let row = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.payload["attempt"], 2);
    assert_eq!(row.published_at, None);
    let lifecycle = Lifecycles::find_by_id(id).one(&db).await.unwrap().unwrap();
    assert_eq!(lifecycle.status, "queued");
    assert_eq!(lifecycle.attempt, 0);
    assert_eq!(lifecycle.provider_job_id, None);
    assert_eq!(
        yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
            .count(&db)
            .await
            .unwrap(),
        1
    );
    db.close().await.unwrap();
}
