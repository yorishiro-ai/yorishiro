//! Tests for `all_tags()` completeness and determinism.

use yorishiro::workers::embedding_sync::WorkerClass;
use yorishiro::workers::queue::all_tags;

/// `all_tags()` must return exactly the union of base worker-class tags and
/// enterprise tags (when compiled in).
///
/// The base set is known at compile time; the enterprise set is one tag when
/// `enterprise` is enabled, zero otherwise.
#[test]
fn all_tags_contains_base_worker_class_tags() {
    let tags = all_tags();
    for class in WorkerClass::ALL {
        let tag = class.tag();
        assert!(
            tags.contains(&tag.to_owned()),
            "all_tags() must include the base tag for {class:?} ({tag})"
        );
    }
}

/// `all_tags()` must contain the enterprise `infer-fill` tag when compiled in.
#[cfg(feature = "enterprise")]
#[test]
fn all_tags_contains_enterprise_infer_fill() {
    let tags = all_tags();
    assert!(
        tags.contains(&"infer-fill".to_owned()),
        "all_tags() must include the enterprise infer-fill tag"
    );
}

/// `all_tags()` must not contain duplicate tags.
#[test]
fn all_tags_has_no_duplicates() {
    let tags = all_tags();
    let mut seen = std::collections::HashSet::new();
    for tag in &tags {
        assert!(
            seen.insert(tag.clone()),
            "duplicate tag in all_tags(): {tag}"
        );
    }
}

/// `all_tags()` output is deterministic (same order across calls).
#[test]
fn all_tags_is_deterministic() {
    let a = all_tags();
    let b = all_tags();
    assert_eq!(
        a, b,
        "all_tags() must produce the same list on repeated calls"
    );
}

/// The number of tags matches the expected count.
#[test]
fn all_tags_count_matches_expected() {
    let tags = all_tags();
    // Base: 3 worker-class tags
    #[allow(unused_mut)]
    let mut expected = WorkerClass::ALL.len();
    #[cfg(feature = "enterprise")]
    {
        // EE adds 1: infer-fill
        expected += 1;
    }
    assert_eq!(
        tags.len(),
        expected,
        "all_tags() returned {} tags but expected {}",
        tags.len(),
        expected
    );
}
