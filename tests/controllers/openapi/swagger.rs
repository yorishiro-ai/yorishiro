use utoipa::openapi::{Components, RefOr, Schema};
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
        &Components::new(),
    );
}

#[test]
#[should_panic(expected = "conflicting OpenAPI component schema")]
fn conflicting_duplicate_schemas_are_rejected() {
    let mut components = Components::new();
    add_schema(
        &mut components,
        "Duplicate".into(),
        utoipa::openapi::schema::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .build()
            .into(),
    );
    add_schema(
        &mut components,
        "Duplicate".into(),
        utoipa::openapi::schema::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::Integer)
            .build()
            .into(),
    );
}

#[test]
fn identical_duplicate_schemas_are_allowed() {
    let mut components = Components::new();
    let schema: RefOr<Schema> = utoipa::openapi::Ref::from_schema_name("Shared").into();
    add_schema(&mut components, "Shared".into(), schema.clone());
    add_schema(&mut components, "Shared".into(), schema);
    assert_eq!(components.schemas.len(), 1);
}
#[cfg(feature = "enterprise")]
mod requests;
