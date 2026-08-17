use crate::error::YorishiroError;

use super::*;

/// Builds an object-typed field nested `depth` levels deep.
fn nested_object(depth: usize) -> serde_json::Value {
    let mut field = serde_json::json!({ "type": "string" });
    for _ in 0..depth {
        field = serde_json::json!({
            "type": "object",
            "properties": { "inner": field }
        });
    }
    serde_json::json!({
        "name": "deep",
        "entity_types": {
            "thing": { "fields": { "root": field } }
        }
    })
}

fn parse(value: serde_json::Value) -> MetaSchemaDefinition {
    serde_json::from_value(value).expect("valid metaschema json")
}

/// `MAX_OBJECT_DEPTH` is the point of the nesting check: a definition may nest object fields up
/// to it, and one level further must be rejected. Asserting the constant is positive proves
/// nothing: this walks the boundary from both sides, so raising or lowering the limit without
/// meaning to fails here.
#[test]
fn object_nesting_is_allowed_up_to_the_limit_and_rejected_past_it() {
    // The check is `depth >= MAX_OBJECT_DEPTH` as the walker descends, so the deepest accepted
    // definition nests one level fewer than the constant.
    validate_definition(&parse(nested_object(MAX_OBJECT_DEPTH - 1)))
        .expect("nesting just inside the limit must be accepted");

    let error = validate_definition(&parse(nested_object(MAX_OBJECT_DEPTH)))
        .expect_err("nesting at the limit must be rejected");

    assert!(matches!(error, YorishiroError::ValidationFailed { .. }));
}

/// The rejection has to say which field was too deep. A bare "invalid schema" leaves the author
/// hunting through a nested definition by hand.
#[test]
fn a_too_deep_definition_reports_the_offending_field() {
    let error = validate_definition(&parse(nested_object(MAX_OBJECT_DEPTH))).unwrap_err();

    let (status, body) = error.into_http_parts();
    assert_eq!(status, 422);

    let rendered = body.to_string();
    assert!(
        rendered.contains("root"),
        "the error should name the field that nests too deeply: {rendered}"
    );
}

/// This module re-exports the metaschema surface under one path, and `validate_definition` is
/// what every write path runs before touching the database. Exercising it here also pins that
/// the re-export still resolves: dropping it stops this file compiling.
#[test]
fn a_definition_with_no_entity_types_is_rejected() {
    let error = validate_definition(&parse(serde_json::json!({
        "name": "empty",
        "entity_types": {}
    })))
    .unwrap_err();

    assert!(matches!(error, YorishiroError::ValidationFailed { .. }));
}

/// `entity_type_to_json_schema` is the projection MCP clients read to learn an entity's shape.
/// A required field must come back as required, or a client will happily send an invalid entity.
#[test]
fn the_json_schema_projection_carries_required_fields_through() {
    let def: MetaSchemaDefinition = parse(serde_json::json!({
        "name": "notes",
        "entity_types": {
            "note": {
                "fields": {
                    "title": { "type": "string", "required": true },
                    "draft": { "type": "boolean" }
                }
            }
        }
    }));

    let projected = entity_type_to_json_schema(def.entity_types.get("note").unwrap());

    assert_eq!(projected["properties"]["title"]["type"], "string");
    let required: Vec<_> = projected["required"]
        .as_array()
        .expect("a type with a required field must project a `required` array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(required.contains(&"title"));
    assert!(!required.contains(&"draft"));
}
