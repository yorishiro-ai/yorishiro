use super::*;

/// Recall is the tool an agent calls to "remember" something, so only the entity is required:
/// every tuning parameter is optional and defaulted downstream.
#[test]
fn only_the_entity_is_required() {
    let args: RecallContextArgs = serde_json::from_value(
        serde_json::json!({ "entity_id": "00000000-0000-0000-0000-000000000000" }),
    )
    .unwrap();

    assert!(args.limit.is_none());
    assert!(args.full.is_none());
    assert!(args.depth.is_none());
}

/// `depth` drives graph traversal; it arrives as a number and must not be silently accepted as a
/// string, which would fail later in the query layer with a much worse message.
#[test]
fn depth_must_be_a_number() {
    assert!(
        serde_json::from_value::<RecallContextArgs>(serde_json::json!({
            "entity_id": "00000000-0000-0000-0000-000000000000",
            "depth": "3"
        }))
        .is_err()
    );

    let args: RecallContextArgs = serde_json::from_value(serde_json::json!({
        "entity_id": "00000000-0000-0000-0000-000000000000",
        "depth": 3
    }))
    .unwrap();
    assert_eq!(args.depth, Some(3));
}
