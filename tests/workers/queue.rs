use chrono::Utc;
use chrono::{Duration, TimeZone};
use migration::{Migrator, MigratorTrait};
use sea_orm::{ColumnTrait, Database, EntityTrait, QueryFilter, sea_query::Expr};
use tempfile::tempdir;
use uuid::Uuid;
use yorishiro::workers::embedding_sync::WorkerClass;

use yorishiro::workers::queue::*;

#[tokio::test]
async fn the_installed_policy_decides_and_a_missing_one_refuses() {
    let ctx = crate::workers::test_context().await;

    let refused = concurrency_for(&ctx, Uuid::nil(), WorkerClass::Shared).await;
    assert!(refused.is_err_and(|error| error.contains("no policy is installed")));

    ctx.shared_store.insert(default_queue_policy());
    for class in [
        WorkerClass::TenantPrivate,
        WorkerClass::Official,
        WorkerClass::Shared,
    ] {
        let policy = concurrency_for(&ctx, Uuid::nil(), class).await.unwrap();
        assert_eq!((policy.plan.as_str(), policy.limit), ("community", 1));
    }
}

async fn database() -> sea_orm::DatabaseConnection {
    let directory = tempdir().expect("queue policy tempdir");
    let path = directory.keep().join("queue-policy.sqlite3");
    let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .expect("queue policy database");
    Migrator::up(&db, None)
        .await
        .expect("queue policy migrations");
    db
}

async fn lifecycle(
    db: &sea_orm::DatabaseConnection,
    class: WorkerClass,
    status: yorishiro::models::queue_job_lifecycles::LifecycleStatus,
    enqueue_at: chrono::DateTime<chrono::FixedOffset>,
) {
    let id = uuid::Uuid::now_v7();
    yorishiro::models::queue_job_lifecycles::Entity::record_enqueue(
        db,
        yorishiro::models::queue_job_lifecycles::Enqueue {
            id,
            job_name: "queue-policy-test",
            worker_class: class,
            workspace_id: None,
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
    )
    .await
    .expect("record queue policy lifecycle");
    yorishiro::models::queue_job_lifecycles::Entity::update_many()
        .col_expr(
            yorishiro::models::queue_job_lifecycles::Column::Status,
            Expr::value(status.as_db_str()),
        )
        .col_expr(
            yorishiro::models::queue_job_lifecycles::Column::EnqueueAt,
            Expr::value(enqueue_at),
        )
        .filter(yorishiro::models::queue_job_lifecycles::Column::Id.eq(id))
        .exec(db)
        .await
        .expect("update queue policy lifecycle");
}

#[test]
fn each_class_is_admitted_at_its_own_band_without_borrowing_another_classs_capacity() {
    assert!(priority(WorkerClass::TenantPrivate) > priority(WorkerClass::Official));
    assert!(priority(WorkerClass::Official) > priority(WorkerClass::Shared));
    for class in [
        WorkerClass::TenantPrivate,
        WorkerClass::Official,
        WorkerClass::Shared,
    ] {
        let decision = decide(class);
        assert_eq!(decision.class, class);
        assert_eq!(decision.priority, priority(class));
        assert!(!decision.fallback);
    }
}

#[tokio::test]
async fn starvation_policy_covers_retrying_candidates_and_exact_boundary() {
    let db = database().await;
    let now = Utc
        .timestamp_opt(100_000, 0)
        .single()
        .unwrap()
        .fixed_offset();
    lifecycle(
        &db,
        WorkerClass::Shared,
        yorishiro::models::queue_job_lifecycles::LifecycleStatus::Retrying,
        now - Duration::seconds(STARVATION_WAIT_SECONDS),
    )
    .await;

    let decision = decide_for_dispatch_at(&db, WorkerClass::Official, now)
        .await
        .unwrap();
    assert_eq!(decision.priority, STARVATION_PRIORITY);
    assert!(decision.fallback);

    let fresh = database().await;
    lifecycle(
        &fresh,
        WorkerClass::Shared,
        yorishiro::models::queue_job_lifecycles::LifecycleStatus::Queued,
        now - Duration::seconds(STARVATION_WAIT_SECONDS - 1),
    )
    .await;
    let decision = decide_for_dispatch_at(&fresh, WorkerClass::Official, now)
        .await
        .unwrap();
    assert_eq!(decision.priority, priority(WorkerClass::Official));
    assert!(!decision.fallback);
}

#[tokio::test]
async fn starvation_policy_excludes_running_and_terminal_rows_and_matches_classes() {
    let db = database().await;
    let now = Utc
        .timestamp_opt(101_000, 0)
        .single()
        .unwrap()
        .fixed_offset();
    for status in [
        yorishiro::models::queue_job_lifecycles::LifecycleStatus::Running,
        yorishiro::models::queue_job_lifecycles::LifecycleStatus::Completed,
        yorishiro::models::queue_job_lifecycles::LifecycleStatus::Failed,
        yorishiro::models::queue_job_lifecycles::LifecycleStatus::Cancelled,
        yorishiro::models::queue_job_lifecycles::LifecycleStatus::Unavailable,
    ] {
        lifecycle(
            &db,
            WorkerClass::Shared,
            status,
            now - Duration::seconds(STARVATION_WAIT_SECONDS + 1),
        )
        .await;
    }
    assert_eq!(
        decide_for_dispatch_at(&db, WorkerClass::Official, now)
            .await
            .unwrap()
            .priority,
        priority(WorkerClass::Official)
    );

    lifecycle(
        &db,
        WorkerClass::Official,
        yorishiro::models::queue_job_lifecycles::LifecycleStatus::Queued,
        now - Duration::seconds(STARVATION_WAIT_SECONDS + 1),
    )
    .await;
    assert_eq!(
        decide_for_dispatch_at(&db, WorkerClass::TenantPrivate, now)
            .await
            .unwrap()
            .priority,
        STARVATION_PRIORITY
    );
    assert_eq!(
        decide_for_dispatch_at(&db, WorkerClass::Shared, now)
            .await
            .unwrap()
            .priority,
        priority(WorkerClass::Shared)
    );
}
