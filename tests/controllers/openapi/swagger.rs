use aide::openapi::{Components, SchemaObject};
use yorishiro::controllers::openapi::swagger::implementation::*;

#[test]
#[should_panic(expected = "missing OpenAPI component schema reference")]
fn missing_schema_references_are_rejected() {
    validate_schema_refs(
        [serde_json::json!({
            "requestBody": {
                "content": {
                    "application/json": {
                        "schema": {
                            "properties": {
                                "nested": {
                                    "$ref": "#/components/schemas/Missing"
                                }
                            }
                        }
                    }
                }
            }
        })],
        &Components::default(),
    );
}

#[test]
#[should_panic(expected = "conflicting OpenAPI component schema")]
fn conflicting_duplicate_schemas_are_rejected() {
    let mut components = Components::default();
    add_schema(
        &mut components,
        "Duplicate".into(),
        schema(schemars::json_schema!({"type": "string"})),
    );
    add_schema(
        &mut components,
        "Duplicate".into(),
        schema(schemars::json_schema!({"type": "integer"})),
    );
}

#[test]
fn identical_duplicate_schemas_are_allowed() {
    let mut components = Components::default();
    let schema = schema(schemars::json_schema!({"type": "string"}));
    add_schema(&mut components, "Shared".into(), schema.clone());
    add_schema(&mut components, "Shared".into(), schema);
    assert_eq!(components.schemas.len(), 1);
}
#[cfg(feature = "enterprise")]
mod requests;

fn schema(json_schema: schemars::Schema) -> SchemaObject {
    SchemaObject {
        json_schema,
        external_docs: None,
        example: None,
    }
}
