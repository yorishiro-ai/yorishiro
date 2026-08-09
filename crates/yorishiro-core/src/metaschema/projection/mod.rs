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
#[path = "../../../tests/metaschema/projection/mod.rs"]
mod tests;
