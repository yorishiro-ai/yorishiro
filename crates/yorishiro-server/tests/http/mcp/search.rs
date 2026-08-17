use super::*;

/// Search is driven by the query text, which is the only required argument. Everything else
/// narrows an already-valid search.
#[test]
fn the_query_text_is_required_and_the_rest_is_optional() {
    assert!(serde_json::from_value::<SearchEntitiesArgs>(serde_json::json!({})).is_err());

    let args: SearchEntitiesArgs =
        serde_json::from_value(serde_json::json!({ "query_text": "how do I deploy" })).unwrap();

    assert_eq!(args.query_text, "how do I deploy");
    assert!(args.entity_type.is_none());
    assert!(args.filter.is_none());
}

/// The filter is a JSONB containment object passed through to the query; it must survive as a
/// structured value rather than being flattened to a string.
#[test]
fn the_filter_is_carried_through_as_structured_json() {
    let args: SearchEntitiesArgs = serde_json::from_value(serde_json::json!({
        "query_text": "anything",
        "filter": { "status": "active", "nested": { "k": 1 } }
    }))
    .unwrap();

    let filter = args.filter.unwrap();
    assert_eq!(filter["status"], "active");
    assert_eq!(filter["nested"]["k"], 1);
}
