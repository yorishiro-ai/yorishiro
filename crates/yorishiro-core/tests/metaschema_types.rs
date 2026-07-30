use serde_json::json;
use yorishiro_core::metaschema::{FieldDef, FieldTypeName};

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

#[test]
fn object_field_with_nested_properties_roundtrips() {
    let field: FieldDef = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "street": { "type": "string", "required": true },
            "city": { "type": "string" }
        }
    }))
    .unwrap();

    assert_eq!(field.r#type, FieldTypeName::Object);
    let properties = field.properties.as_ref().unwrap();
    assert!(properties["street"].required);
    assert!(!properties["city"].required);

    let roundtripped = serde_json::to_value(&field).unwrap();
    assert_eq!(roundtripped["properties"]["street"]["type"], "string");
    assert_eq!(roundtripped["properties"]["city"]["type"], "string");
}
