use super::*;

/// The export format is a tagged union so a reader can tell record kinds apart without tracking
/// line position. The tag and content keys are the on-disk format -- pinned here because a
/// rename would make every previously exported file unreadable.
#[test]
fn each_kind_is_tagged_so_a_reader_can_discriminate_lines() {
    let entity = ExportRecord::Entity(crate::models::entities::EntityRecord {
        id: uuid::Uuid::nil(),
        workspace_id: uuid::Uuid::nil(),
        schema_id: uuid::Uuid::nil(),
        schema_version: 1,
        entity_type: "task".into(),
        data: serde_json::json!({ "title": "t" }),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        created_by: None,
        updated_by: None,
    });

    let json = serde_json::to_value(&entity).unwrap();

    assert_eq!(json["kind"], "entity");
    assert_eq!(json["record"]["entity_type"], "task");
}

/// Import reads back exactly what export wrote, so the round trip through the tagged form has to
/// preserve which variant a line was.
#[test]
fn a_record_round_trips_back_into_the_same_variant() {
    let relation = ExportRecord::Relation(crate::models::relations::RelationRecord {
        id: uuid::Uuid::nil(),
        workspace_id: uuid::Uuid::nil(),
        source_id: uuid::Uuid::nil(),
        target_id: uuid::Uuid::nil(),
        relation_type: "blocks".into(),
        properties: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    });

    let encoded = serde_json::to_string(&relation).unwrap();
    let decoded: ExportRecord = serde_json::from_str(&encoded).unwrap();

    assert!(matches!(decoded, ExportRecord::Relation(_)));
}
