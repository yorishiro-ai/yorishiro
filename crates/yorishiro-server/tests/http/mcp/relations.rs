use super::*;

/// A relation needs both endpoints and a type; properties are optional because most relations carry none.
#[test]
fn creating_a_relation_requires_both_endpoints_and_a_type() {
    assert!(
        serde_json::from_value::<CreateRelationArgs>(serde_json::json!({
            "source_id": "00000000-0000-0000-0000-000000000000"
        }))
        .is_err()
    );

    let args: CreateRelationArgs = serde_json::from_value(serde_json::json!({
        "source_id": "00000000-0000-0000-0000-000000000000",
        "target_id": "00000000-0000-0000-0000-000000000000",
        "relation_type": "depends_on"
    }))
    .unwrap();

    assert_eq!(args.relation_type, "depends_on");
    assert!(args.properties.is_none());
}

/// Listing filters are all optional: an agent asking for "the relations here" sends nothing.
#[test]
fn listing_relations_accepts_an_empty_object() {
    let args: ListRelationsArgs = serde_json::from_value(serde_json::json!({})).unwrap();

    assert!(args.source_id.is_none());
    assert!(args.target_id.is_none());
}
