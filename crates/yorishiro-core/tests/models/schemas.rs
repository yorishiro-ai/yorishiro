use super::*;

/// `SchemaSummary` is the listing shape MCP clients read first, so it must stay lightweight --
/// specifically it must not carry the `definition` body, which is the whole reason it exists.
#[test]
fn a_summary_omits_the_definition_body() {
    let summary = SchemaSummary {
        id: uuid::Uuid::nil(),
        name: "task-management".into(),
        version: 3,
        status: "active".into(),
        created_at: chrono::Utc::now(),
    };

    let json = serde_json::to_value(&summary).unwrap();

    assert!(json.get("definition").is_none());
    assert_eq!(json["name"], "task-management");
    assert_eq!(json["version"], 3);
    assert_eq!(json["status"], "active");
}

/// A schema record travels through a JSONL export and back, carrying its parsed definition. The
/// round trip is what `repositories::import` relies on.
#[test]
fn a_schema_record_round_trips_with_its_parsed_definition() {
    let definition: crate::metaschema::MetaSchemaDefinition =
        serde_json::from_value(serde_json::json!({
            "name": "task-management",
            "entity_types": {
                "task": {
                    "fields": {
                        "title": { "type": "string", "required": true }
                    }
                }
            },
            "relation_types": {}
        }))
        .unwrap();

    let record = SchemaRecord {
        id: uuid::Uuid::nil(),
        tenant_id: uuid::Uuid::nil(),
        workspace_id: uuid::Uuid::nil(),
        name: "task-management".into(),
        version: 1,
        definition,
        status: "active".into(),
        origin_template_id: None,
        origin_status: "detached".to_string(),
        origin_snapshot: None,
        created_at: chrono::Utc::now(),
    };

    let encoded = serde_json::to_string(&record).unwrap();
    let decoded: SchemaRecord = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded.name, record.name);
    assert_eq!(decoded.status, record.status);
    assert!(decoded.definition.entity_types.contains_key("task"));
}
