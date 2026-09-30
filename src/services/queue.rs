//! Queue scheduling policy shared by every Loco queue provider.

use chrono::Utc;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};

use crate::workers::embedding_sync::WorkerClass;

/// The provider priority bands used by all three supported Loco providers.
///
/// Loco defines larger values as more urgent and resolves ties by `run_at`, then
/// by the stable provider job id. Keeping a band per class means the provider
/// does the ordering, while this module owns the application policy.
pub(crate) const fn priority(class: WorkerClass) -> i32 {
    match class {
        WorkerClass::TenantPrivate => 300,
        WorkerClass::Official => 200,
        WorkerClass::Shared => 100,
    }
}

const AGING_INTERVAL_SECONDS: i64 = 60;
const MAX_AGING_STEPS: i64 = 2;
const STARVATION_PRIORITY: i32 = 50;

/// A queue decision recorded for operators and deterministic unit tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) class: WorkerClass,
    pub(crate) priority: i32,
    pub(crate) fallback: bool,
}

/// Adds at most two bounded aging steps to an older retry.
///
/// A shared job reaches priority 300 after two minutes, which ties the
/// tenant-private band and wins that tie through the provider's older
/// `run_at` ordering. The cap prevents an old job from permanently reversing
/// the class policy.
pub(crate) fn decide_after_wait(class: WorkerClass, waited_seconds: i64) -> Decision {
    let steps = (waited_seconds / AGING_INTERVAL_SECONDS).clamp(0, MAX_AGING_STEPS);
    Decision {
        class,
        priority: priority(class) + (steps as i32 * 100),
        fallback: steps > 0,
    }
}

/// Recomputes priority from the durable lifecycle enqueue timestamp on retry.
pub(crate) async fn decide_for_lifecycle(
    db: &sea_orm::DatabaseConnection,
    lifecycle_id: uuid::Uuid,
    class: WorkerClass,
) -> Result<Decision, sea_orm::DbErr> {
    let lifecycle = crate::models::queue_job_lifecycles::Entity::find_by_id(lifecycle_id)
        .one(db)
        .await?
        .ok_or_else(|| sea_orm::DbErr::RecordNotFound("queue lifecycle".into()))?;
    let waited_seconds = Utc::now()
        .signed_duration_since(lifecycle.enqueue_at)
        .num_seconds()
        .max(0);
    Ok(decide_after_wait(class, waited_seconds))
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
    let lower_classes: &[&str] = match class {
        WorkerClass::TenantPrivate => &["official", "shared"],
        WorkerClass::Official => &["shared"],
        WorkerClass::Shared => &[],
    };
    if lower_classes.is_empty() {
        return Ok(decide(class));
    }
    let cutoff = Utc::now() - chrono::Duration::seconds(AGING_INTERVAL_SECONDS);
    let waiting = crate::models::queue_job_lifecycles::Entity::find()
        .filter(crate::models::queue_job_lifecycles::Column::Status.eq("queued"))
        .filter(
            crate::models::queue_job_lifecycles::Column::WorkerClass
                .is_in(lower_classes.iter().copied()),
        )
        .filter(crate::models::queue_job_lifecycles::Column::EnqueueAt.lte(cutoff))
        .count(db)
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
    decide_after_wait(class, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn aging_is_bounded_and_promotes_older_lower_classes() {
        assert_eq!(decide_after_wait(WorkerClass::Shared, 0).priority, 100);
        assert_eq!(decide_after_wait(WorkerClass::Shared, 120).priority, 300);
        assert_eq!(decide_after_wait(WorkerClass::Shared, 600).priority, 300);
        assert!(decide_after_wait(WorkerClass::Shared, 60).fallback);
    }
}
