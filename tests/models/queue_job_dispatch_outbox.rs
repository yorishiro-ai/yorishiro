use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, PaginatorTrait, QueryFilter,
    TransactionTrait,
};
use uuid::Uuid;
use yorishiro::models::queue_job_dispatch_outbox::{due, record};
use yorishiro::models::queue_job_lifecycles::{Enqueue, Entity as Lifecycles, LifecycleStatus};
use yorishiro::workers::embedding_sync::WorkerClass;

#[tokio::test]
async fn acknowledgement_preserves_correlation_after_consumer_starts() {
    use yorishiro::models::queue_job_dispatch_outbox::{acknowledge_dispatch, claim_dispatch};
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let id = Uuid::now_v7();
        record(
            &ctx.db,
            Enqueue {
                id,
                job_name: "ack-race",
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
        let claim = Uuid::now_v7();
        assert!(claim_dispatch(&ctx.db, id, claim, 60).await.unwrap());
        let admission = Lifecycles::start_at(&ctx.db, id, Utc::now().fixed_offset())
            .await
            .unwrap();
        assert!(matches!(
            admission,
            yorishiro::models::queue_job_lifecycles::Admission::Started { .. }
        ));
        let before = Lifecycles::find_by_id(id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert!(
            acknowledge_dispatch(&ctx.db, id, claim, "provider-ack-race")
                .await
                .unwrap()
        );
        let after = Lifecycles::find_by_id(id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.provider_job_id.as_deref(), Some("provider-ack-race"));
        assert_eq!(after.status, before.status);
        assert_eq!(after.attempt, before.attempt);
        let outbox =
            yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
        assert!(outbox.published_at.is_some());
        assert!(
            !acknowledge_dispatch(&ctx.db, id, claim, "stale-ack")
                .await
                .unwrap()
        );
        assert_eq!(
            Lifecycles::find_by_id(id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .provider_job_id
                .as_deref(),
            Some("provider-ack-race")
        );
    })
    .await;
}

#[tokio::test]
async fn record_is_recoverable_until_attempted() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|_request, ctx| async move {
        let db = ctx.db.clone();
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
            serde_json::json!({"value": 7, "jsonl": "sensitive-import-body"}),
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
        let stored =
            yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
                .one(&db)
                .await
                .unwrap()
                .unwrap();
        let diagnostic = format!("{stored:?}");
        assert!(diagnostic.contains("[REDACTED]"));
        assert!(!diagnostic.contains("sensitive-import-body"));
        assert_eq!(stored.payload["jsonl"], "sensitive-import-body");
    })
    .await;
}

#[tokio::test]
async fn durable_claim_excludes_a_second_relay_and_requires_matching_ack_token() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let db = ctx.db.clone();
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
        assert!(matches!(
            yorishiro::models::queue_job_lifecycles::Entity::start_at(
                &db,
                id,
                chrono::Utc::now().fixed_offset()
            )
            .await
            .unwrap(),
            yorishiro::models::queue_job_lifecycles::Admission::Started { .. }
        ));
        assert!(
            yorishiro::models::queue_job_dispatch_outbox::acknowledge_dispatch(
                &db,
                id,
                first,
                &format!("provider-{id}")
            )
            .await
            .unwrap()
        );
    })
    .await;
}

#[tokio::test]
async fn recovery_eligibility_uses_lifecycle_status_and_retry_at() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|_request, ctx| async move {
        let db = ctx.db.clone();

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

                match LifecycleStatus::from_row(status).unwrap() {
                    LifecycleStatus::Queued => {}
                    LifecycleStatus::Retrying => {
                        Lifecycles::defer(&db, id, None, "retry eligibility")
                            .await
                            .unwrap();
                    }
                    LifecycleStatus::Running => {
                        assert!(matches!(
                            Lifecycles::start_at(&db, id, now).await.unwrap(),
                            yorishiro::models::queue_job_lifecycles::Admission::Started {
                                attempt: 1
                            }
                        ));
                    }
                    terminal => {
                        Lifecycles::finish(&db, id, None, terminal, None)
                            .await
                            .unwrap();
                    }
                }
                // Inject clock offsets and lost/stale provider acknowledgements for recovery eligibility.
                let mut lifecycle = Lifecycles::find_by_id(id)
                    .one(&db)
                    .await
                    .unwrap()
                    .unwrap()
                    .into_active_model();
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
    })
    .await;
}

#[tokio::test]
async fn ordinary_retry_eligibility_uses_retry_at_not_provider_job_id() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|_request, ctx| async move {
        let db = ctx.db.clone();
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
        Lifecycles::defer(&db, due_id, None, "provider acknowledgement lost")
            .await
            .unwrap();
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
        Lifecycles::defer(&db, late_id, None, "retry later")
            .await
            .unwrap();
        let mut late = Lifecycles::find_by_id(late_id)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
        // Inject only the future retry clock and prior publication state under test.
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
    })
    .await;
}

#[tokio::test]
async fn requeue_updates_the_existing_outbox_and_clears_provider_ack() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let db = ctx.db.clone();
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
        for expected_attempt in 1..=3 {
            let admission = Lifecycles::start_at(&db, id, Utc::now().fixed_offset())
                .await
                .unwrap();
            assert!(matches!(
                admission,
                yorishiro::models::queue_job_lifecycles::Admission::Started { attempt }
                    if attempt == expected_attempt
            ));
            Lifecycles::defer(&db, id, Some(expected_attempt), "retry")
                .await
                .unwrap();
        }
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
                .filter(yorishiro::models::_entities::queue_job_dispatch_outbox::Column::Id.eq(id))
                .count(&db)
                .await
                .unwrap(),
            1
        );
    })
    .await;
}
