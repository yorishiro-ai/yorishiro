use super::*;

/// The default list query must not filter anything out: a caller that supplies no parameters expects the whole workspace, one page at a time.
#[test]
fn the_default_list_query_filters_nothing_and_uses_the_documented_limit() {
    let query = ListEntitiesQuery::default();

    assert!(query.entity_type.is_none());
    assert!(query.filter.is_none());
    assert_eq!(query.limit, DEFAULT_LIST_LIMIT);
    assert_eq!(query.offset, 0);
}

/// `EntityRecord` is written to a JSONL export and read back by `repositories::import`, so the round trip is a real code path rather than a theoretical one.
/// `data` is free-form JSON and has to survive verbatim.
#[test]
fn an_entity_record_round_trips_through_json_with_its_data_intact() {
    let record = EntityRecord {
        id: uuid::Uuid::nil(),
        workspace_id: uuid::Uuid::nil(),
        schema_id: uuid::Uuid::nil(),
        schema_version: 2,
        entity_type: "task".into(),
        data: serde_json::json!({
            "title": "write the test",
            "done": false,
            "tags": ["a", "b"],
            "nested": { "depth": 1 }
        }),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        created_by: None,
        updated_by: None,
    };

    let encoded = serde_json::to_string(&record).unwrap();
    let decoded: EntityRecord = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded.entity_type, record.entity_type);
    assert_eq!(decoded.schema_version, record.schema_version);
    assert_eq!(decoded.data, record.data);
}

/// An entity written through a service key has no user attached; those columns must serialise as null rather than being dropped, so a re-import keeps the same attribution.
#[test]
fn unattributed_entities_keep_explicit_nulls_for_their_actor_columns() {
    let record = EntityRecord {
        id: uuid::Uuid::nil(),
        workspace_id: uuid::Uuid::nil(),
        schema_id: uuid::Uuid::nil(),
        schema_version: 1,
        entity_type: "task".into(),
        data: serde_json::json!({}),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        created_by: None,
        updated_by: None,
    };

    let json = serde_json::to_value(&record).unwrap();

    assert!(json.get("created_by").is_some());
    assert!(json["created_by"].is_null());
    assert!(json["updated_by"].is_null());
}
