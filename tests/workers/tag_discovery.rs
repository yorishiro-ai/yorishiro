//! The worker registry is the single source of tags, queues and registration.

use yorishiro::edition::{worker_tags, workers};
use yorishiro::workers::embedding_sync::WorkerClass;
use yorishiro::workers::registry::WorkerRegistry;

fn community_tags() -> Vec<String> {
    WorkerClass::ALL
        .iter()
        .map(|class| class.tag().to_owned())
        .collect()
}

/// The community registry names exactly the worker classes' tags and queues, whichever edition is compiled in.
#[test]
fn community_registry_is_the_worker_class_baseline() {
    let registry = WorkerRegistry::community();
    assert_eq!(registry.tags(), community_tags());
    assert_eq!(registry.queues(), community_tags());
}

/// Without the enterprise feature the composed registry is the community one: nothing else contributes.
#[cfg(not(feature = "enterprise"))]
#[test]
fn composed_registry_is_the_community_registry_without_an_edition() {
    assert_eq!(worker_tags(), community_tags());
    assert_eq!(workers().queues(), community_tags());
}

/// With the enterprise feature the composed registry is the community baseline followed by exactly the edition's own workers.
#[cfg(feature = "enterprise")]
#[test]
fn composed_registry_extends_the_community_baseline_only() {
    let mut expected = community_tags();
    expected.push("infer-fill".to_owned());
    assert_eq!(worker_tags(), expected);
    assert_eq!(workers().queues(), expected);
    assert_eq!(
        &worker_tags()[..WorkerClass::ALL.len()],
        WorkerRegistry::community().tags().as_slice(),
        "the edition must not reorder or replace the baseline"
    );
}

/// Tags are listed once each and in a stable order, since the list is substituted verbatim into `--worker=`.
#[test]
fn composed_tags_are_distinct_and_deterministic() {
    let tags = worker_tags();
    let mut seen = std::collections::HashSet::new();
    assert!(
        tags.iter().all(|tag| seen.insert(tag)),
        "duplicate tag in {tags:?}"
    );
    assert_eq!(tags, worker_tags());
}

/// Registering the same worker type twice is refused, because Loco keys handlers by class name and the second would replace the first.
#[test]
#[should_panic(expected = "duplicate background worker")]
fn registering_a_worker_twice_is_refused() {
    use yorishiro::workers::embedding_sync::{EmbeddingSyncArgs, EmbeddingSyncWorkerShared};
    let _ = WorkerRegistry::community().register::<EmbeddingSyncArgs, EmbeddingSyncWorkerShared>();
}
