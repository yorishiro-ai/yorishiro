use super::*;

/// `ListRelationsQuery::default()` is what a caller gets when it omits every filter, so the
/// defaults are the de-facto API contract for an unfiltered list.
#[test]
fn the_default_list_query_filters_nothing_and_uses_the_documented_limit() {
    let query = ListRelationsQuery::default();

    assert!(query.source_id.is_none());
    assert!(query.target_id.is_none());
    assert!(query.relation_type.is_none());
    assert_eq!(query.limit, DEFAULT_LIST_LIMIT);
    assert_eq!(query.offset, 0);
}

/// `RelationRecord` is both written to the API and read back from a JSONL export, so it has to
/// survive a serialize/deserialize round trip unchanged -- including the free-form `properties`.
#[test]
fn a_relation_record_round_trips_through_json() {
    let record = RelationRecord {
        id: uuid::Uuid::nil(),
        workspace_id: uuid::Uuid::nil(),
        source_id: uuid::Uuid::nil(),
        target_id: uuid::Uuid::nil(),
        relation_type: "depends_on".into(),
        properties: serde_json::json!({ "weight": 3, "note": "manual" }),
        status: "active".to_string(),
        created_at: chrono::Utc::now(),
    };

    let encoded = serde_json::to_string(&record).unwrap();
    let decoded: RelationRecord = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded.relation_type, record.relation_type);
    assert_eq!(decoded.properties, record.properties);
    assert_eq!(decoded.source_id, record.source_id);
    assert_eq!(decoded.target_id, record.target_id);
}
