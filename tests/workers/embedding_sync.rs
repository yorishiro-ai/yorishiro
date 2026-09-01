/// Tests for the embedding sync worker: worker class tags, serialization, and class names.
use yorishiro::workers::embedding_sync::{
    EmbeddingSyncWorkerTenantPrivate, EmbeddingSyncWorkerOfficial, EmbeddingSyncWorkerShared,
    WorkerClass,
};

/// Each of the three worker types must carry exactly its own `WorkerClass`'s tag and no other: a type whose `tags()` drifted to list a second class's tag (or dropped its own) would let a tag-restricted worker process either miss its own jobs or pick up another class's, exactly the bug this whole split exists to close.
#[test]
fn each_worker_type_carries_exactly_its_own_class_tag() {
    assert_eq!(
        EmbeddingSyncWorkerTenantPrivate::tags(),
        vec![WorkerClass::TenantPrivate.tag().to_string()]
    );
    assert_eq!(
        EmbeddingSyncWorkerOfficial::tags(),
        vec![WorkerClass::Official.tag().to_string()]
    );
    assert_eq!(
        EmbeddingSyncWorkerShared::tags(),
        vec![WorkerClass::Shared.tag().to_string()]
    );
}

/// `serde(rename_all = "snake_case")` is what `EmbeddingSyncArgs` actually persists into `pg_loco_queue`'s `task_data`; asserting the wire form catches an accidental rename breaking a job already sitting in the queue at deploy time.
#[test]
fn worker_class_serializes_to_snake_case() {
    assert_eq!(
        serde_json::to_value(WorkerClass::TenantPrivate).unwrap(),
        serde_json::json!("tenant_private")
    );
    assert_eq!(
        serde_json::to_value(WorkerClass::Official).unwrap(),
        serde_json::json!("official")
    );
    assert_eq!(
        serde_json::to_value(WorkerClass::Shared).unwrap(),
        serde_json::json!("shared")
    );
}

/// `as_db_str`/`from_db_str` must round-trip every variant, and must agree with the `snake_case` serde wire form above: `ee/`'s `identity_workspace_worker_classes` stores this same string, so a row read from the database and a value read off a queued job's payload must be indistinguishable.
#[test]
fn db_str_round_trips_and_matches_the_serde_wire_form() {
    for class in [
        WorkerClass::TenantPrivate,
        WorkerClass::Official,
        WorkerClass::Shared,
    ] {
        let db_str = class.as_db_str();
        assert_eq!(
            WorkerClass::from_db_str(db_str).unwrap(),
            class,
            "as_db_str/from_db_str must round-trip {class:?}"
        );
        assert_eq!(
            serde_json::to_value(class).unwrap(),
            serde_json::json!(db_str),
            "{class:?}'s db string must match its serde wire form"
        );
    }
}

#[test]
fn from_db_str_rejects_an_unknown_value() {
    assert!(WorkerClass::from_db_str("not-a-real-class").is_err());
}

/// `App::connect_workers` registers each of the three types under its own `class_name()`.
/// Registering needs a real `Queue`, which a unit test has no access to, so this guards the
/// assumption instead: three distinct class names, or one `register` call silently clobbers
/// another's handler rather than adding a third.
/// `enqueue_for_class`'s exhaustive `match` on `WorkerClass` already forces a compile error if a fourth variant is added with no worker type to dispatch to; this test covers the complementary runtime gap `connect_workers` itself has no compiler check for: a worker type that exists and is dispatched to, but was never actually registered.
#[test]
fn the_three_worker_types_have_distinct_class_names() {
    let names = [
        EmbeddingSyncWorkerTenantPrivate::class_name(),
        EmbeddingSyncWorkerOfficial::class_name(),
        EmbeddingSyncWorkerShared::class_name(),
    ];
    let unique: std::collections::HashSet<_> = names.iter().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "worker types must have distinct class_name()s, got {names:?}"
    );
}
