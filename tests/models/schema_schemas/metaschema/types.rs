use serde_json::json;
/// Tests for metaschema type definitions: `FieldDef` serialization, x- attributes, and round-tripping.
use yorishiro::models::schema_schemas::metaschema::FieldDef;

#[test]
fn preserves_known_and_unknown_x_attributes() {
    let field: FieldDef = serde_json::from_value(json!({
        "type": "string",
        "x-embed": true,
        "x-ui": { "widget": "select", "options": ["a", "b"] },
        "x-custom-client-hint": { "anything": 1 }
    }))
    .unwrap();

    assert!(field.x_embed);
    assert_eq!(
        field.x_ui,
        Some(json!({ "widget": "select", "options": ["a", "b"] }))
    );
    assert_eq!(
        field.extra.get("x-custom-client-hint"),
        Some(&json!({ "anything": 1 }))
    );

    let roundtripped = serde_json::to_value(&field).unwrap();
    assert_eq!(
        roundtripped["x-custom-client-hint"],
        json!({ "anything": 1 })
    );
}
