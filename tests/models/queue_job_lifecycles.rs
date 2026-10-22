use chrono::TimeZone;
use chrono::Utc;
use sea_orm::{Database, EntityTrait};
use yorishiro::models::_entities::queue_job_lifecycles::Entity;

use yorishiro::models::queue_job_lifecycles::{
    Admission, Enqueue, LifecycleStatus, lease_duration,
};

#[tokio::test]
async fn the_database_accepts_every_status_the_enum_defines() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let concurrency_key = uuid::Uuid::now_v7().to_string();
        use sea_orm::{ColumnTrait, QueryFilter};

        let db = ctx.db.clone();
        let id = uuid::Uuid::now_v7();
        enqueue(&db, id, None, &concurrency_key).await;
        for status in LifecycleStatus::ALL {
            // Exercise the database CHECK independently of lifecycle transition guards.
            Entity::update_many()
                .col_expr(
                    yorishiro::models::queue_job_lifecycles::Column::Status,
                    sea_orm::sea_query::Expr::value(status.as_db_str()),
                )
                .filter(yorishiro::models::queue_job_lifecycles::Column::Id.eq(id))
                .exec(&db)
                .await
                .unwrap_or_else(|error| panic!("CHECK constraint rejected {status}: {error}"));
        }
    })
    .await;
}

#[test]
fn lifecycle_status_values_round_trip() {
    for status in LifecycleStatus::ALL {
        assert_eq!(LifecycleStatus::from_row(status.as_db_str()), Ok(*status));
    }
    assert!(LifecycleStatus::from_row("unknown").is_err());
}

async fn enqueue(
    db: &sea_orm::DatabaseConnection,
    id: uuid::Uuid,
    limit: Option<i32>,
    concurrency_key: &str,
) {
    Entity::record_enqueue(
        db,
        Enqueue {
            id,
            job_name: "test",
            worker_class: yorishiro::workers::embedding_sync::WorkerClass::Shared,
            workspace_id: None,
            plan: None,
            concurrency_key: Some(concurrency_key),
            concurrency_limit: limit,
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn timezone_state_changes_without_rewriting_lifecycle_instants() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|_request, ctx| async move {
        let concurrency_key = uuid::Uuid::now_v7().to_string();
        let db = ctx.db.clone();
        let id = uuid::Uuid::now_v7();
        enqueue(&db, id, None, &concurrency_key).await;
        let before = Entity::find_by_id(id).one(&db).await.unwrap().unwrap();

        yorishiro::models::timezone_state::activate(&db, "Asia/Tokyo")
            .await
            .unwrap();
        yorishiro::models::timezone_state::activate(&db, "Asia/Tokyo")
            .await
            .unwrap();
        let after = Entity::find_by_id(id).one(&db).await.unwrap().unwrap();
        assert_eq!(before.enqueue_at, after.enqueue_at);
        let state = yorishiro::models::timezone_state::Entity::find_by_id(1)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .active_timezone;
        assert_eq!(state, "Asia/Tokyo");
    })
    .await;
}

#[tokio::test]
async fn timezone_state_rejects_invalid_and_missing_singleton_without_inserting() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|_request, ctx| async move {
        let db = ctx.db.clone();
        assert!(
            yorishiro::models::timezone_state::activate(&db, "Invalid/Zone")
                .await
                .is_err()
        );
        // Inject a missing singleton to verify activation fails rather than silently recreating it.
        yorishiro::models::_entities::timezone_state::Entity::delete_many()
            .exec(&db)
            .await
            .unwrap();
        assert!(
            yorishiro::models::timezone_state::activate(&db, "Asia/Tokyo")
                .await
                .is_err()
        );
    })
    .await;
}

#[test]
fn live_lease_is_duplicate_but_expired_lease_is_recoverable() {
    let now = Utc.timestamp_opt(2_000, 0).single().unwrap().fixed_offset();
    assert!(
        yorishiro::models::queue_job_lifecycles::running_lease_is_live(
            Some(now + chrono::Duration::seconds(1)),
            now
        )
    );
    assert!(
        !yorishiro::models::queue_job_lifecycles::running_lease_is_live(
            Some(now - chrono::Duration::seconds(1)),
            now
        )
    );
    assert!(yorishiro::models::queue_job_lifecycles::running_lease_is_live(None, now));
}

#[tokio::test]
async fn admission_is_attempt_fenced_and_heartbeat_is_deterministic() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let concurrency_key = uuid::Uuid::now_v7().to_string();
        let db = ctx.db.clone();
        let id = uuid::Uuid::now_v7();
        enqueue(&db, id, None, &concurrency_key).await;
        let now = Utc.timestamp_opt(3_000, 0).single().unwrap().fixed_offset();
        assert_eq!(
            Entity::start_at(&db, id, now).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        assert!(matches!(
            Entity::start_at(&db, id, now).await.unwrap(),
            Admission::Duplicate { .. }
        ));
        assert!(Entity::renew_at(&db, id, 1, now).await.unwrap());
        let row = Entity::find_by_id(id).one(&db).await.unwrap().unwrap();
        assert_eq!(row.lease_until, Some(now + lease_duration()));
        assert!(!Entity::renew_at(&db, id, 2, now).await.unwrap());
    })
    .await;
}

#[tokio::test]
async fn expired_attempt_recovers_and_stale_completion_is_rejected() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let concurrency_key = uuid::Uuid::now_v7().to_string();
        let db = ctx.db.clone();
        let id = uuid::Uuid::now_v7();
        enqueue(&db, id, None, &concurrency_key).await;
        let first = Utc.timestamp_opt(4_000, 0).single().unwrap().fixed_offset();
        assert_eq!(
            Entity::start_at(&db, id, first).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        let second = first + lease_duration() + chrono::Duration::seconds(1);
        assert_eq!(
            Entity::start_at(&db, id, second).await.unwrap(),
            Admission::Recovered { attempt: 2 }
        );
        assert!(
            Entity::finish(
                &db,
                id,
                Some(1),
                yorishiro::models::queue_job_lifecycles::LifecycleStatus::Completed,
                None
            )
            .await
            .is_err()
        );
        assert!(
            Entity::finish(
                &db,
                id,
                Some(2),
                yorishiro::models::queue_job_lifecycles::LifecycleStatus::Completed,
                None
            )
            .await
            .is_ok()
        );
    })
    .await;
}

#[tokio::test]
async fn concurrent_admission_allows_one_attempt() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let concurrency_key = uuid::Uuid::now_v7().to_string();
        let first = ctx.db.clone();
        let second = Database::connect(&ctx.config.database.uri).await.unwrap();
        let id = uuid::Uuid::now_v7();
        enqueue(&first, id, None, &concurrency_key).await;
        let now = Utc.timestamp_opt(5_000, 0).single().unwrap().fixed_offset();
        let (left, right) = tokio::join!(
            Entity::start_at(&first, id, now),
            Entity::start_at(&second, id, now)
        );
        second.close().await.unwrap();
        let admissions = [left.unwrap(), right.unwrap()];
        let started = admissions
            .iter()
            .filter(|a| matches!(a, Admission::Started { .. } | Admission::Recovered { .. }))
            .count();
        assert_eq!(started, 1);
    })
    .await;
}

#[tokio::test]
async fn saturation_defers_without_consuming_or_duplicating_the_retry() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let concurrency_key = uuid::Uuid::now_v7().to_string();
        let db = ctx.db.clone();
        let active = uuid::Uuid::now_v7();
        let waiting = uuid::Uuid::now_v7();
        enqueue(&db, active, Some(1), &concurrency_key).await;
        enqueue(&db, waiting, Some(1), &concurrency_key).await;
        let now = Utc.timestamp_opt(6_000, 0).single().unwrap().fixed_offset();
        assert_eq!(
            Entity::start_at(&db, active, now).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        assert_eq!(
            Entity::start_at(&db, waiting, now).await.unwrap(),
            Admission::Saturated { attempt: None }
        );
        Entity::defer(&db, waiting, None, "capacity").await.unwrap();
        let row = Entity::find_by_id(waiting).one(&db).await.unwrap().unwrap();
        assert_eq!(row.status, "retrying");
        assert!(row.lease_until.is_none());
        assert_eq!(
            Entity::start_at(&db, waiting, now).await.unwrap(),
            Admission::Saturated { attempt: None }
        );
        let row = Entity::find_by_id(waiting).one(&db).await.unwrap().unwrap();
        assert_eq!(row.status, "retrying");
        assert_eq!(row.attempt, 0);
    })
    .await;
}

#[tokio::test]
async fn expired_saturation_defers_with_its_attempt_and_recovers_later() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let concurrency_key = uuid::Uuid::now_v7().to_string();
        let db = ctx.db.clone();
        let active = uuid::Uuid::now_v7();
        let expired = uuid::Uuid::now_v7();
        enqueue(&db, active, Some(1), &concurrency_key).await;
        enqueue(&db, expired, Some(1), &concurrency_key).await;
        let first = Utc.timestamp_opt(7_000, 0).single().unwrap().fixed_offset();
        assert_eq!(
            Entity::start_at(&db, expired, first).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        let expired_at = first + lease_duration() + chrono::Duration::seconds(1);
        assert_eq!(
            Entity::start_at(&db, active, expired_at).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        assert_eq!(
            Entity::start_at(&db, expired, expired_at).await.unwrap(),
            Admission::Saturated { attempt: Some(1) }
        );
        Entity::defer_at(&db, expired, 1, "capacity", expired_at)
            .await
            .unwrap();
        let row = Entity::find_by_id(expired).one(&db).await.unwrap().unwrap();
        assert_eq!(row.status, "retrying");
        assert_eq!(row.attempt, 1);
        assert!(
            Entity::finish(
                &db,
                expired,
                Some(1),
                yorishiro::models::queue_job_lifecycles::LifecycleStatus::Completed,
                None
            )
            .await
            .is_err()
        );
        Entity::finish(
            &db,
            active,
            Some(1),
            yorishiro::models::queue_job_lifecycles::LifecycleStatus::Completed,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            Entity::start_at(&db, expired, expired_at).await.unwrap(),
            Admission::Started { attempt: 2 }
        );
    })
    .await;
}

#[tokio::test]
async fn renewed_expired_saturation_cannot_be_deferred() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let concurrency_key = uuid::Uuid::now_v7().to_string();
        let db = ctx.db.clone();
        let active = uuid::Uuid::now_v7();
        let expired = uuid::Uuid::now_v7();
        enqueue(&db, active, Some(1), &concurrency_key).await;
        enqueue(&db, expired, Some(1), &concurrency_key).await;
        let first = Utc.timestamp_opt(8_000, 0).single().unwrap().fixed_offset();
        assert_eq!(
            Entity::start_at(&db, expired, first).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        let observed = first + lease_duration() + chrono::Duration::seconds(1);
        assert_eq!(
            Entity::start_at(&db, active, observed).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        assert_eq!(
            Entity::start_at(&db, expired, observed).await.unwrap(),
            Admission::Saturated { attempt: Some(1) }
        );
        let renewed = observed + chrono::Duration::seconds(1);
        assert!(Entity::renew_at(&db, expired, 1, renewed).await.unwrap());
        assert!(
            Entity::defer_at(&db, expired, 1, "capacity", observed)
                .await
                .is_err()
        );
        let row = Entity::find_by_id(expired).one(&db).await.unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert_eq!(row.attempt, 1);
        assert!(row.lease_until.unwrap() > observed);
        assert!(matches!(
            Entity::start_at(&db, expired, renewed).await.unwrap(),
            Admission::Duplicate { attempt: 1 }
        ));
    })
    .await;
}

#[tokio::test]
async fn live_attempt_failure_defers_without_changing_attempt() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let concurrency_key = uuid::Uuid::now_v7().to_string();
        let db = ctx.db.clone();
        let id = uuid::Uuid::now_v7();
        enqueue(&db, id, None, &concurrency_key).await;
        let now = Utc.timestamp_opt(9_000, 0).single().unwrap().fixed_offset();
        assert_eq!(
            Entity::start_at(&db, id, now).await.unwrap(),
            Admission::Started { attempt: 1 }
        );
        Entity::defer(&db, id, Some(1), "worker failed")
            .await
            .unwrap();
        let row = Entity::find_by_id(id).one(&db).await.unwrap().unwrap();
        assert_eq!(row.status, "retrying");
        assert_eq!(row.attempt, 1);
        assert!(Entity::defer(&db, id, Some(0), "stale").await.is_err());
        assert_eq!(
            Entity::find_by_id(id)
                .one(&db)
                .await
                .unwrap()
                .unwrap()
                .status,
            "retrying"
        );
    })
    .await;
}
