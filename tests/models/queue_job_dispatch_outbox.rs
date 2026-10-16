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

    let mut row = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    row.last_attempt_at = sea_orm::ActiveValue::Set(None);
    row.update(&db).await.unwrap();
    let publish_txn = db.begin().await.unwrap();
    assert!(
        yorishiro::models::queue_job_dispatch_outbox::claim_for_publish(&publish_txn, id, 1)
            .await
            .unwrap()
    );
    publish_txn.commit().await.unwrap();
    let mut row = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    row.published_at = sea_orm::ActiveValue::Set(Some(Utc::now().fixed_offset()));
    row.last_attempt_at = sea_orm::ActiveValue::Set(None);
    row.update(&db).await.unwrap();
    let txn = db.begin().await.unwrap();
    assert!(
        !yorishiro::models::queue_job_dispatch_outbox::claim_for_publish(&txn, id, 1)
            .await
            .unwrap()
    );
    txn.rollback().await.unwrap();
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
        yorishiro::models::queue_job_dispatch_outbox::claim_dispatch(&db, id, 1, first, 30)
            .await
            .unwrap()
    );
    assert!(
        !yorishiro::models::queue_job_dispatch_outbox::claim_dispatch(&db, id, 1, second, 30)
            .await
            .unwrap()
    );
    assert!(
        !yorishiro::models::queue_job_dispatch_outbox::acknowledge_dispatch(
            &db,
            id,
            1,
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
            1,
            first,
            "provider-1"
        )
        .await
        .unwrap()
    );
    db.close().await.unwrap();
}

#[tokio::test]
async fn paused_epoch_rejects_claim_and_retagged_row_rejects_late_ack() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::connect(format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("claim-fence.sqlite3").display()
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
            job_name: "claim-fence",
            worker_class: WorkerClass::Shared,
            workspace_id: Some(workspace_id),
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
        serde_json::json!({"workspace_id": workspace_id.to_string()}),
        "TestWorker".into(),
        None,
        None,
    )
    .await
    .unwrap();
    let token = Uuid::now_v7();
    assert!(
        yorishiro::models::queue_job_dispatch_outbox::claim_dispatch(&db, id, 1, token, 30)
            .await
            .unwrap()
    );
    let txn = db.begin().await.unwrap();
    let paused = yorishiro::models::queue_job_dispatch_outbox::advance_publishing_epoch(
        &txn, 1, false, None,
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    assert!(
        !yorishiro::models::queue_job_dispatch_outbox::claim_dispatch(
            &db,
            id,
            1,
            Uuid::now_v7(),
            30
        )
        .await
        .unwrap()
    );
    let txn = db.begin().await.unwrap();
    yorishiro::models::queue_job_dispatch_outbox::prepare_replay_snapshot(
        &txn,
        paused,
        Utc::now().fixed_offset(),
        true,
        "snapshot-race",
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    assert!(
        !yorishiro::models::queue_job_dispatch_outbox::acknowledge_dispatch(
            &db,
            id,
            1,
            token,
            "provider-old"
        )
        .await
        .unwrap()
    );
    let lifecycle = Lifecycles::find_by_id(id).one(&db).await.unwrap().unwrap();
    assert_eq!(lifecycle.provider_job_id, None);
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
            "attempt": 2,
            "dispatch_id": Uuid::now_v7().to_string(),
            "publishing_epoch": -99
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
    assert_eq!(row.payload["dispatch_id"], id.to_string());
    assert_eq!(row.payload["publishing_epoch"], row.publishing_epoch);
    assert_eq!(row.publishing_epoch, 1);
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

#[tokio::test]
async fn cutover_fences_old_deliveries_and_replay_gets_a_new_generation() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::connect(format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("epoch.sqlite3").display()
    ))
    .await
    .unwrap();
    Migrator::up(&db, None).await.unwrap();
    let id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
    let query_id = Uuid::now_v7();
    let query_request_id = Uuid::now_v7();
    let tenant = yorishiro::models::_entities::tenant_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("replay-snapshot-test".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    yorishiro::models::_entities::workspace_workspaces::ActiveModel {
        id: sea_orm::ActiveValue::Set(workspace_id),
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set("replay-snapshot-test".into()),
        status: sea_orm::ActiveValue::Set("active".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    let query_txn = db.begin().await.unwrap();
    let query_deadline = Utc::now().fixed_offset() + chrono::Duration::hours(1);
    yorishiro::models::query_embedding_requests::ActiveModel {
        id: sea_orm::ActiveValue::Set(query_request_id),
        workspace_id: sea_orm::ActiveValue::Set(workspace_id),
        query_text: sea_orm::ActiveValue::Set("pending accepted query".into()),
        status: sea_orm::ActiveValue::Set("pending".into()),
        expires_at: sea_orm::ActiveValue::Set(query_deadline),
        deadline_at: sea_orm::ActiveValue::Set(Some(query_deadline)),
        ..Default::default()
    }
    .insert(&query_txn)
    .await
    .unwrap();
    yorishiro::models::queue_job_dispatch_outbox::record_query_in_transaction(
        &query_txn,
        query_id,
        query_deadline,
        serde_json::json!({"request_id": query_request_id, "workspace_id": workspace_id.to_string()}),
        "QueryEmbeddingWorker".into(),
        Some(yorishiro::workers::target::WorkerTarget::Shared.queue()),
        Some(vec![yorishiro::workers::target::WorkerTarget::Shared.tag()]),
    )
    .await
    .unwrap();
    query_txn.commit().await.unwrap();
    let query_row =
        yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(query_id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(query_row.priority, Some(1000));
    assert_ne!(query_id, query_request_id);
    assert_eq!(query_row.workspace_id, Some(workspace_id));
    assert_eq!(
        query_row.queue_name.as_deref(),
        Some("worker-target:shared")
    );
    assert_eq!(
        query_row.tags,
        Some(serde_json::json!(["worker-target:shared"]))
    );
    assert!(
        yorishiro::models::queue_job_dispatch_outbox::due(&db)
            .await
            .unwrap()
            .iter()
            .any(|row| row.id == query_id)
    );
    let mut query_row = query_row.into_active_model();
    query_row.published_at = sea_orm::ActiveValue::Set(Some(Utc::now().fixed_offset()));
    query_row.update(&db).await.unwrap();

    let expired_query_id = Uuid::now_v7();
    let expired_query_txn = db.begin().await.unwrap();
    yorishiro::models::query_embedding_requests::ActiveModel {
        id: sea_orm::ActiveValue::Set(expired_query_id),
        workspace_id: sea_orm::ActiveValue::Set(workspace_id),
        query_text: sea_orm::ActiveValue::Set("expired query".into()),
        status: sea_orm::ActiveValue::Set("expired".into()),
        expires_at: sea_orm::ActiveValue::Set(query_deadline),
        deadline_at: sea_orm::ActiveValue::Set(Some(query_deadline)),
        ..Default::default()
    }
    .insert(&expired_query_txn)
    .await
    .unwrap();
    yorishiro::models::queue_job_dispatch_outbox::record_query_in_transaction(
        &expired_query_txn,
        expired_query_id,
        query_deadline,
        serde_json::json!({"request_id": expired_query_id, "workspace_id": workspace_id.to_string()}),
        "QueryEmbeddingWorker".into(),
        Some(yorishiro::workers::target::WorkerTarget::Shared.queue()),
        Some(vec![yorishiro::workers::target::WorkerTarget::Shared.tag()]),
    )
    .await
    .unwrap();
    expired_query_txn.commit().await.unwrap();
    let mut expired_query_outbox =
        yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(
            expired_query_id,
        )
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    expired_query_outbox.published_at = sea_orm::ActiveValue::Set(Some(Utc::now().fixed_offset()));
    expired_query_outbox.update(&db).await.unwrap();
    yorishiro::models::queue_job_dispatch_outbox::record(
        &db,
        Enqueue {
            id,
            job_name: "cutover-test",
            worker_class: WorkerClass::Shared,
            workspace_id: Some(workspace_id),
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
        serde_json::json!({"workspace_id": workspace_id.to_string()}),
        "TestWorker".into(),
        Some("shared".into()),
        None,
    )
    .await
    .unwrap();
    let old = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(
        yorishiro::models::queue_job_dispatch_outbox::delivery_epoch_allowed(
            &db,
            Some(id),
            Some(id),
            Some(old.publishing_epoch)
        )
        .await
        .unwrap()
    );

    let txn = db.begin().await.unwrap();
    let next = yorishiro::models::queue_job_dispatch_outbox::change_publishing_state(
        &txn,
        old.publishing_epoch,
        true,
        None,
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    assert!(
        !yorishiro::models::queue_job_dispatch_outbox::delivery_epoch_allowed(
            &db,
            None,
            Some(id),
            Some(old.publishing_epoch)
        )
        .await
        .unwrap()
    );
    assert_ne!(next, old.publishing_epoch);
    let pause_txn = db.begin().await.unwrap();
    let paused_epoch = yorishiro::models::queue_job_dispatch_outbox::change_publishing_state(
        &pause_txn, next, false, None,
    )
    .await
    .unwrap();
    yorishiro::models::queue_job_dispatch_outbox::set_cutover_cutoff(
        &pause_txn,
        paused_epoch,
        Utc::now().fixed_offset(),
    )
    .await
    .unwrap();
    pause_txn.commit().await.unwrap();
    let replay_txn = db.begin().await.unwrap();
    let replayed = yorishiro::models::queue_job_dispatch_outbox::prepare_replay_snapshot(
        &replay_txn,
        paused_epoch,
        Utc::now().fixed_offset(),
        true,
        "provider-snapshot-test",
    )
    .await
    .unwrap();
    assert_eq!(replayed, 2);
    let prepared_in_txn =
        yorishiro::models::queue_job_dispatch_outbox::publishing_state(&replay_txn)
            .await
            .unwrap();
    assert_eq!(prepared_in_txn.epoch, paused_epoch + 1);
    let row_in_txn =
        yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
            .one(&replay_txn)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(row_in_txn.publishing_epoch, paused_epoch + 1);
    replay_txn.commit().await.unwrap();
    let replay = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.publishing_epoch, paused_epoch + 1);
    assert_eq!(replay.payload["publishing_epoch"], paused_epoch + 1);
    let replay_query =
        yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(query_id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(replay_query.publishing_epoch, paused_epoch + 1);
    assert_eq!(replay_query.published_at, None);
    let expired_replay_query =
        yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(
            expired_query_id,
        )
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expired_replay_query.publishing_epoch, 1);
    assert!(expired_replay_query.published_at.is_some());
    let prepared_state = yorishiro::models::queue_job_dispatch_outbox::publishing_state(&db)
        .await
        .unwrap();
    assert_eq!(prepared_state.epoch, paused_epoch + 1);
    assert!(!prepared_state.active);
    assert_eq!(
        prepared_state.snapshot_id.as_deref(),
        Some("provider-snapshot-test")
    );
    assert!(
        !yorishiro::models::queue_job_dispatch_outbox::delivery_epoch_allowed(
            &db,
            Some(id),
            Some(id),
            Some(paused_epoch + 1)
        )
        .await
        .unwrap()
    );
    let active_txn = db.begin().await.unwrap();
    yorishiro::models::queue_job_dispatch_outbox::activate_prepared_publishing_epoch(
        &active_txn,
        paused_epoch,
        "provider-snapshot-test",
    )
    .await
    .unwrap();
    active_txn.commit().await.unwrap();
    assert!(
        yorishiro::models::queue_job_dispatch_outbox::delivery_epoch_allowed(
            &db,
            Some(id),
            Some(id),
            Some(paused_epoch + 1)
        )
        .await
        .unwrap()
    );
    db.close().await.unwrap();
}

#[tokio::test]
async fn planned_replay_keeps_future_retry_and_reclaims_running_regardless_of_lease() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::connect(format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("planned-replay.sqlite3").display()
    ))
    .await
    .unwrap();
    Migrator::up(&db, None).await.unwrap();
    let now = Utc::now().fixed_offset();
    let future = now + chrono::Duration::days(1);
    let workspace_id = Uuid::now_v7();
    let mut ids = Vec::new();
    for (status, lease_until) in [
        ("retrying", None),
        ("running", None),
        ("running", Some(now - chrono::Duration::minutes(1))),
        ("running", Some(future)),
    ] {
        let id = Uuid::now_v7();
        record(
            &db,
            Enqueue {
                id,
                job_name: "planned-replay",
                worker_class: WorkerClass::Shared,
                workspace_id: Some(workspace_id),
                plan: None,
                concurrency_key: None,
                concurrency_limit: None,
            },
            serde_json::json!({"value": status, "workspace_id": workspace_id}),
            "TestWorker".into(),
            Some("test-queue".into()),
            None,
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
        lifecycle.retry_at = sea_orm::ActiveValue::Set((status == "retrying").then_some(future));
        lifecycle.lease_until = sea_orm::ActiveValue::Set(lease_until);
        lifecycle.update(&db).await.unwrap();
        if status == "running" {
            let mut outbox =
                yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
                    .one(&db)
                    .await
                    .unwrap()
                    .unwrap()
                    .into_active_model();
            outbox.published_at = sea_orm::ActiveValue::Set(Some(now));
            outbox.update(&db).await.unwrap();
        }
        ids.push(id);
    }
    let pause = db.begin().await.unwrap();
    let paused = yorishiro::models::queue_job_dispatch_outbox::change_publishing_state(
        &pause, 1, false, None,
    )
    .await
    .unwrap();
    yorishiro::models::queue_job_dispatch_outbox::set_cutover_cutoff(
        &pause,
        paused,
        Utc::now().fixed_offset(),
    )
    .await
    .unwrap();
    pause.commit().await.unwrap();
    let replay_txn = db.begin().await.unwrap();
    assert!(
        yorishiro::models::queue_job_dispatch_outbox::prepare_replay_snapshot(
            &replay_txn,
            paused,
            Utc::now().fixed_offset(),
            false,
            "snapshot"
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("attestation")
    );
    replay_txn.rollback().await.unwrap();
    let replay_txn = db.begin().await.unwrap();
    let replay_count = yorishiro::models::queue_job_dispatch_outbox::prepare_replay_snapshot(
        &replay_txn,
        paused,
        Utc::now().fixed_offset(),
        true,
        "snapshot-42",
    )
    .await
    .unwrap();
    assert_eq!(replay_count, 4);
    replay_txn.commit().await.unwrap();
    let next_epoch = paused + 1;
    let queued_outbox =
        yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(ids[1])
            .one(&db)
            .await
            .unwrap()
            .unwrap();
    let future_retry_outbox =
        yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(ids[0])
            .one(&db)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(queued_outbox.publishing_epoch, next_epoch);
    assert_eq!(future_retry_outbox.publishing_epoch, next_epoch);
    let snapshot_rows = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
        .all(&db)
        .await
        .unwrap();
    assert_eq!(
        snapshot_rows
            .iter()
            .filter(|row| ids.contains(&row.id) && row.publishing_epoch == next_epoch)
            .count(),
        4
    );
    let retry = Lifecycles::find_by_id(ids[0])
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.retry_at, Some(future));
    assert_eq!(retry.status, "retrying");
    let retry_outbox =
        yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(ids[0])
            .one(&db)
            .await
            .unwrap()
            .unwrap();
    let active_epoch = retry_outbox.publishing_epoch;
    let pending = yorishiro::models::queue_job_dispatch_outbox::due(&db)
        .await
        .unwrap();
    assert!(!pending.iter().any(|row| row.id == ids[0]));
    let mut retry = retry.into_active_model();
    retry.retry_at = sea_orm::ActiveValue::Set(Some(now - chrono::Duration::seconds(1)));
    retry.update(&db).await.unwrap();
    let mut retry_outbox =
        yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(ids[0])
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
    retry_outbox.published_at = sea_orm::ActiveValue::Set(None);
    retry_outbox.update(&db).await.unwrap();
    assert_eq!(
        due(&db)
            .await
            .unwrap()
            .iter()
            .filter(|row| row.id == ids[0])
            .count(),
        1
    );
    assert_eq!(
        due(&db)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.id == ids[0])
            .unwrap()
            .publishing_epoch,
        active_epoch
    );
    let activate = db.begin().await.unwrap();
    yorishiro::models::queue_job_dispatch_outbox::activate_prepared_publishing_epoch(
        &activate,
        active_epoch - 1,
        "snapshot-42",
    )
    .await
    .unwrap();
    activate.commit().await.unwrap();
    let running = Lifecycles::find_by_id(ids[3])
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(running.status, "queued");
    assert_eq!(running.lease_until, None);
    db.close().await.unwrap();
}
