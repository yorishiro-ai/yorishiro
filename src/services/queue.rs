//! Queue scheduling policy shared by every Loco queue provider.

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

/// A queue decision recorded for operators and deterministic unit tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) class: WorkerClass,
    pub(crate) priority: i32,
    pub(crate) fallback: bool,
}

/// Selects a class priority without consulting wall-clock time.
///
/// Capacity is deliberately not borrowed between classes. When a preferred
/// class is full, its job remains queued for that class instead of silently
/// consuming another class's reserved capacity. The independent class worker
/// is the starvation guard: work in another class can continue while this job
/// waits.
pub(crate) const fn decide(class: WorkerClass) -> Decision {
    Decision {
        class,
        priority: priority(class),
        fallback: false,
    }
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
}
