use crate::metaschema::{MetaSchemaDefinition, entity_type_to_json_schema};
use serde_json::json;

#[test]
fn projects_required_and_enum_fields() {
    let def: MetaSchemaDefinition = serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "project": {
                "fields": {
                    "title": { "type": "string", "required": true, "x-embed": true },
                    "status": { "type": "string", "enum": ["active", "done"], "required": true }
                }
            }
        }
    }))
    .unwrap();

    let schema = entity_type_to_json_schema(&def.entity_types["project"]);
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["title"]["type"], "string");
    assert_eq!(schema["properties"]["status"]["enum"][0], "active");

    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&json!("title")));
    assert!(required.contains(&json!("status")));
}

#[test]
fn projects_array_field_with_string_items() {
    let def: MetaSchemaDefinition = serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "tags": { "type": "array", "items": { "type": "string" } }
                }
            }
        }
    }))
    .unwrap();

    let schema = entity_type_to_json_schema(&def.entity_types["task"]);
    assert_eq!(schema["properties"]["tags"]["type"], "array");
    assert_eq!(schema["properties"]["tags"]["items"]["type"], "string");
}

#[test]
fn projects_object_field_with_nested_properties_and_required() {
    let def: MetaSchemaDefinition = serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "address": {
                        "type": "object",
                        "properties": {
                            "street": { "type": "string", "required": true },
                            "city": { "type": "string" }
                        }
                    }
                }
            }
        }
    }))
    .unwrap();

    let schema = entity_type_to_json_schema(&def.entity_types["task"]);
    let address = &schema["properties"]["address"];
    assert_eq!(address["type"], "object");
    assert_eq!(address["properties"]["street"]["type"], "string");
    assert_eq!(address["properties"]["city"]["type"], "string");
    assert_eq!(address["required"].as_array().unwrap(), &[json!("street")]);
}

#[test]
fn projects_array_field_with_object_items() {
    let def: MetaSchemaDefinition = serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "contacts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string", "required": true }
                            }
                        }
                    }
                }
            }
        }
    }))
    .unwrap();

    let schema = entity_type_to_json_schema(&def.entity_types["task"]);
    let items = &schema["properties"]["contacts"]["items"];
    assert_eq!(items["type"], "object");
    assert_eq!(items["properties"]["name"]["type"], "string");
    assert_eq!(items["required"].as_array().unwrap(), &[json!("name")]);
}
