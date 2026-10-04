use chrono::TimeZone;
use chrono::Utc;
use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, EntityTrait, Statement};
use tempfile::tempdir;
use yorishiro::models::_entities::queue_job_lifecycles::Entity;

use yorishiro::models::queue_job_lifecycles::{
    Admission, Enqueue, LifecycleStatus, lease_duration,
};

#[tokio::test]
async fn the_database_accepts_every_status_the_enum_defines() {
    use sea_orm::{ColumnTrait, QueryFilter};

    let db = database().await;
    let id = uuid::Uuid::now_v7();
    enqueue(&db, id, None).await;
    for status in LifecycleStatus::ALL {
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
}

#[test]
fn lifecycle_status_values_round_trip() {
    for status in LifecycleStatus::ALL {
        assert_eq!(LifecycleStatus::from_row(status.as_db_str()), Ok(*status));
    }
    assert!(LifecycleStatus::from_row("unknown").is_err());
}

async fn database() -> sea_orm::DatabaseConnection {
    let path = tempdir().unwrap().keep().join("queue.sqlite3");
    let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    Migrator::up(&db, None).await.unwrap();
    db
}

async fn enqueue(db: &sea_orm::DatabaseConnection, id: uuid::Uuid, limit: Option<i32>) {
    Entity::record_enqueue(
        db,
        Enqueue {
            id,
            job_name: "test",
            worker_class: yorishiro::workers::embedding_sync::WorkerClass::Shared,
            workspace_id: None,
            plan: None,
            concurrency_key: Some("shared"),
            concurrency_limit: limit,
        },
    )
    .await
    .unwrap();
}

#[test]
fn queue_start_latency_uses_recorded_timestamps() {
    let enqueue_at = Utc.timestamp_opt(1_000, 0).single().unwrap().fixed_offset();
    let start_at = Utc.timestamp_opt(1_075, 0).single().unwrap().fixed_offset();
    let row = yorishiro::models::queue_job_lifecycles::Model {
        id: uuid::Uuid::nil(),
        provider_job_id: None,
        job_name: "job".into(),
        worker_class: "shared".into(),
        workspace_id: None,
        plan: None,
        status: "running".into(),
        enqueue_at,
        claim_at: Some(start_at),
        start_at: Some(start_at),
        retry_at: None,
        completed_at: None,
        failed_at: None,
        cancelled_at: None,
        admitted_at: Some(start_at),
        lease_until: Some(start_at + chrono::Duration::minutes(5)),
        attempt: 1,
        concurrency_key: None,
        concurrency_limit: None,
        error: None,
        created_at: enqueue_at,
        updated_at: start_at,
    };
    assert_eq!(
        row.start_at
            .map(|start| (start - row.enqueue_at).num_seconds()),
        Some(75)
    );
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
    let db = database().await;
    let id = uuid::Uuid::now_v7();
    enqueue(&db, id, None).await;
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
}

#[tokio::test]
async fn expired_attempt_recovers_and_stale_completion_is_rejected() {
    let db = database().await;
    let id = uuid::Uuid::now_v7();
    enqueue(&db, id, None).await;
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
}

#[tokio::test]
async fn concurrent_admission_allows_one_attempt() {
    let directory = tempdir().unwrap();
    let uri = format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("queue.sqlite3").display()
    );
    let first = Database::connect(&uri).await.unwrap();
    let second = Database::connect(&uri).await.unwrap();
    Migrator::up(&first, None).await.unwrap();
    for db in [&first, &second] {
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;",
        ))
        .await
        .unwrap();
    }
    let id = uuid::Uuid::now_v7();
    enqueue(&first, id, None).await;
    let now = Utc.timestamp_opt(5_000, 0).single().unwrap().fixed_offset();
    let (left, right) = tokio::join!(
        Entity::start_at(&first, id, now),
        Entity::start_at(&second, id, now)
    );
    let admissions = [left.unwrap(), right.unwrap()];
    let started = admissions
        .iter()
        .filter(|a| matches!(a, Admission::Started { .. } | Admission::Recovered { .. }))
        .count();
    assert_eq!(started, 1);
}

#[tokio::test]
async fn saturation_defers_without_consuming_or_duplicating_the_retry() {
    let db = database().await;
    let active = uuid::Uuid::now_v7();
    let waiting = uuid::Uuid::now_v7();
    enqueue(&db, active, Some(1)).await;
    enqueue(&db, waiting, Some(1)).await;
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
}

#[tokio::test]
async fn expired_saturation_defers_with_its_attempt_and_recovers_later() {
    let db = database().await;
    let active = uuid::Uuid::now_v7();
    let expired = uuid::Uuid::now_v7();
    enqueue(&db, active, Some(1)).await;
    enqueue(&db, expired, Some(1)).await;
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
}

#[tokio::test]
async fn renewed_expired_saturation_cannot_be_deferred() {
    let db = database().await;
    let active = uuid::Uuid::now_v7();
    let expired = uuid::Uuid::now_v7();
    enqueue(&db, active, Some(1)).await;
    enqueue(&db, expired, Some(1)).await;
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
}

#[tokio::test]
async fn live_attempt_failure_defers_without_changing_attempt() {
    let db = database().await;
    let id = uuid::Uuid::now_v7();
    enqueue(&db, id, None).await;
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
}
