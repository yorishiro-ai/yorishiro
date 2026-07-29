use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::types::{EntityTypeDef, FieldDef, FieldTypeName};

/// Builds a JSON Schema from an EntityTypeDef that's used both for validating
/// entity `data` and generating the MCP inputSchema.
/// The metaschema is the sole source for this schema; adapters only consume the result.
pub fn entity_type_to_json_schema(entity_type: &EntityTypeDef) -> Value {
    properties_to_json_schema(&entity_type.fields)
}

fn properties_to_json_schema(fields: &BTreeMap<String, FieldDef>) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();

    for (field_name, field) in fields {
        properties.insert(field_name.clone(), field_to_json_schema(field));
        if field.required {
            required.push(Value::String(field_name.clone()));
        }
    }

    let mut schema = json!({
        "type": "object",
        "properties": properties,
    });

    if !required.is_empty() {
        schema["required"] = Value::Array(required);
    }

    schema
}

fn field_to_json_schema(field: &FieldDef) -> Value {
    let mut schema = Map::new();

    let type_str = match field.r#type {
        FieldTypeName::String => "string",
        FieldTypeName::Number => "number",
        FieldTypeName::Integer => "integer",
        FieldTypeName::Boolean => "boolean",
        FieldTypeName::Array => "array",
        FieldTypeName::Object => "object",
    };
    schema.insert("type".into(), Value::String(type_str.into()));

    if let Some(description) = &field.description {
        schema.insert("description".into(), Value::String(description.clone()));
    }
    if let Some(enum_values) = &field.enum_values {
        schema.insert(
            "enum".into(),
            Value::Array(enum_values.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(format) = &field.format {
        schema.insert("format".into(), Value::String(format.clone()));
    }
    if let Some(minimum) = field.minimum {
        schema.insert("minimum".into(), json!(minimum));
    }
    if let Some(maximum) = field.maximum {
        schema.insert("maximum".into(), json!(maximum));
    }
    if let Some(min_length) = field.min_length {
        schema.insert("minLength".into(), json!(min_length));
    }
    if let Some(max_length) = field.max_length {
        schema.insert("maxLength".into(), json!(max_length));
    }
    if let Some(pattern) = &field.pattern {
        schema.insert("pattern".into(), Value::String(pattern.clone()));
    }
    if let Some(min_items) = field.min_items {
        schema.insert("minItems".into(), json!(min_items));
    }
    if let Some(max_items) = field.max_items {
        schema.insert("maxItems".into(), json!(max_items));
    }
    if field.unique_items {
        schema.insert("uniqueItems".into(), json!(true));
    }
    if let Some(default) = &field.default {
        schema.insert("default".into(), default.clone());
    }
    if matches!(field.r#type, FieldTypeName::Array)
        && let Some(items) = &field.items
    {
        let items_schema = if items.r#type == "object" {
            let mut items_schema =
                properties_to_json_schema(items.properties.as_ref().unwrap_or(&BTreeMap::new()));
            items_schema["type"] = Value::String("object".into());
            items_schema
        } else {
            json!({ "type": items.r#type })
        };
        schema.insert("items".into(), items_schema);
    }

    if matches!(field.r#type, FieldTypeName::Object)
        && let Some(properties) = &field.properties
    {
        let nested = properties_to_json_schema(properties);
        schema.insert("properties".into(), nested["properties"].clone());
        if let Some(required) = nested.get("required") {
            schema.insert("required".into(), required.clone());
        }
    }

    Value::Object(schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metaschema::types::MetaSchemaDefinition;
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
}
