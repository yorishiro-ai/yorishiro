//! Queue scheduling policy shared by every Loco queue provider.

use chrono::Utc;

use crate::workers::embedding_sync::WorkerClass;

const TENANT_PRIVATE_PRIORITY: i32 = 300;
const OFFICIAL_PRIORITY: i32 = 200;
const SHARED_PRIORITY: i32 = 100;

/// The provider priority bands used by all three supported Loco providers.
///
/// Loco defines larger values as more urgent and resolves ties by `run_at`, then
/// by the stable provider job id. Keeping a band per class means the provider
/// does the ordering, while this module owns the application policy.
pub(crate) const fn priority(class: WorkerClass) -> i32 {
    match class {
        WorkerClass::TenantPrivate => TENANT_PRIVATE_PRIORITY,
        WorkerClass::Official => OFFICIAL_PRIORITY,
        WorkerClass::Shared => SHARED_PRIORITY,
    }
}

const STARVATION_WAIT_SECONDS: i64 = 60;
const STARVATION_PRIORITY: i32 = 50;

/// A queue decision recorded for operators and deterministic unit tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) class: WorkerClass,
    pub(crate) priority: i32,
    pub(crate) fallback: bool,
}

/// Demotes a newly arriving higher class while an older lower class is waiting.
///
/// Loco does not expose a portable reprioritization operation for an already
/// queued job, so this boundary protects a waiting job by making subsequent
/// higher-class arrivals yield to it. The one-minute threshold and bounded
/// priority band make the result deterministic without a new queue provider.
pub(crate) async fn decide_for_dispatch(
    db: &sea_orm::DatabaseConnection,
    class: WorkerClass,
) -> Result<Decision, sea_orm::DbErr> {
    decide_for_dispatch_at(db, class, Utc::now().fixed_offset()).await
}

async fn decide_for_dispatch_at(
    db: &sea_orm::DatabaseConnection,
    class: WorkerClass,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> Result<Decision, sea_orm::DbErr> {
    let lower_classes: &[WorkerClass] = match class {
        WorkerClass::TenantPrivate => &[WorkerClass::Official, WorkerClass::Shared],
        WorkerClass::Official => &[WorkerClass::Shared],
        WorkerClass::Shared => &[],
    };
    if lower_classes.is_empty() {
        return Ok(decide(class));
    }
    let cutoff = now - chrono::Duration::seconds(STARVATION_WAIT_SECONDS);
    let waiting = crate::models::queue_job_lifecycles::Entity::count_waiting_lower_classes(
        db,
        lower_classes,
        cutoff,
    )
    .await?;
    Ok(if waiting > 0 {
        Decision {
            class,
            priority: STARVATION_PRIORITY,
            fallback: true,
        }
    } else {
        decide(class)
    })
}

/// Selects the initial class priority without consulting wall-clock time.
///
/// Capacity is deliberately not borrowed between classes. When a preferred
/// class is full, its job remains queued for that class instead of silently
/// consuming another class's reserved capacity. The independent class worker
/// is the starvation guard: work in another class can continue while this job
/// waits.
pub(crate) fn decide(class: WorkerClass) -> Decision {
    Decision {
        class,
        priority: priority(class),
        fallback: false,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};
    use migration::{Migrator, MigratorTrait};
    use sea_orm::{ColumnTrait, Database, EntityTrait, QueryFilter, sea_query::Expr};
    use tempfile::tempdir;

    use super::*;

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
        status: crate::models::queue_job_lifecycles::LifecycleStatus,
        enqueue_at: chrono::DateTime<chrono::FixedOffset>,
    ) {
        let id = uuid::Uuid::now_v7();
        crate::models::queue_job_lifecycles::Entity::record_enqueue(
            db,
            crate::models::queue_job_lifecycles::Enqueue {
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
        crate::models::queue_job_lifecycles::Entity::update_many()
            .col_expr(
                crate::models::queue_job_lifecycles::Column::Status,
                Expr::value(status.as_str()),
            )
            .col_expr(
                crate::models::queue_job_lifecycles::Column::EnqueueAt,
                Expr::value(enqueue_at),
            )
            .filter(crate::models::queue_job_lifecycles::Column::Id.eq(id))
            .exec(db)
            .await
            .expect("update queue policy lifecycle");
    }

    #[test]
    fn class_order_and_equal_input_tie_breaking_are_stable() {
        assert!(priority(WorkerClass::TenantPrivate) > priority(WorkerClass::Official));
        assert!(priority(WorkerClass::Official) > priority(WorkerClass::Shared));
        assert_eq!(decide(WorkerClass::Shared), decide(WorkerClass::Shared));
    }

    #[test]
    fn capacity_does_not_fallback_to_another_class() {
        let decision = decide(WorkerClass::Official);
        assert_eq!(decision.class, WorkerClass::Official);
        assert!(!decision.fallback);
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
            crate::models::queue_job_lifecycles::LifecycleStatus::Retrying,
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
            crate::models::queue_job_lifecycles::LifecycleStatus::Queued,
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
            crate::models::queue_job_lifecycles::LifecycleStatus::Running,
            crate::models::queue_job_lifecycles::LifecycleStatus::Completed,
            crate::models::queue_job_lifecycles::LifecycleStatus::Failed,
            crate::models::queue_job_lifecycles::LifecycleStatus::Cancelled,
            crate::models::queue_job_lifecycles::LifecycleStatus::Unavailable,
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
            crate::models::queue_job_lifecycles::LifecycleStatus::Queued,
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
}
