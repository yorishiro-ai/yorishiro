use serde_json::json;
use yorishiro_core::YorishiroError;
use yorishiro_core::metaschema::{MAX_OBJECT_DEPTH, MetaSchemaDefinition, validate_definition};

fn parse(value: serde_json::Value) -> MetaSchemaDefinition {
    serde_json::from_value(value).expect("valid metaschema json")
}

#[test]
fn accepts_well_formed_definition() {
    let def = parse(json!({
        "name": "task-management",
        "entity_types": {
            "project": { "fields": { "title": { "type": "string", "required": true } } },
            "task": { "fields": { "title": { "type": "string", "required": true } } }
        },
        "relation_types": {
            "belongs_to": { "source": "task", "target": "project" }
        }
    }));
    assert!(validate_definition(&def).is_ok());
}

#[test]
fn rejects_unknown_relation_target() {
    let def = parse(json!({
        "name": "task-management",
        "entity_types": {
            "task": { "fields": { "title": { "type": "string" } } }
        },
        "relation_types": {
            "belongs_to": { "source": "task", "target": "project" }
        }
    }));
    let err = validate_definition(&def).unwrap_err();
    match err {
        YorishiroError::ValidationFailed { details, .. } => {
            assert!(details.iter().any(|d| d.field.contains("target")));
        }
        _ => panic!("expected ValidationFailed"),
    }
}

#[test]
fn rejects_array_field_without_string_items() {
    let def = parse(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "tags": { "type": "array", "items": { "type": "number" } }
                }
            }
        }
    }));
    assert!(validate_definition(&def).is_err());
}

#[test]
fn rejects_empty_entity_types() {
    let def = parse(json!({ "name": "empty", "entity_types": {} }));
    assert!(validate_definition(&def).is_err());
}

#[test]
fn rejects_format_on_non_string_field() {
    let def = parse(json!({
        "name": "task-management",
        "entity_types": {
            "task": { "fields": { "count": { "type": "integer", "format": "date" } } }
        }
    }));
    let err = validate_definition(&def).unwrap_err();
    match err {
        YorishiroError::ValidationFailed { details, .. } => {
            assert!(details.iter().any(|d| d.field.ends_with("/format")));
        }
        _ => panic!("expected ValidationFailed"),
    }
}

#[test]
fn rejects_minimum_on_boolean_field() {
    let def = parse(json!({
        "name": "task-management",
        "entity_types": {
            "task": { "fields": { "done": { "type": "boolean", "minimum": 0 } } }
        }
    }));
    assert!(validate_definition(&def).is_err());
}

#[test]
fn rejects_minimum_greater_than_maximum() {
    let def = parse(json!({
        "name": "task-management",
        "entity_types": {
            "task": { "fields": { "score": { "type": "integer", "minimum": 10, "maximum": 1 } } }
        }
    }));
    assert!(validate_definition(&def).is_err());
}

#[test]
fn rejects_object_field_without_properties() {
    let def = parse(json!({
        "name": "task-management",
        "entity_types": {
            "task": { "fields": { "address": { "type": "object" } } }
        }
    }));
    assert!(validate_definition(&def).is_err());
}

#[test]
fn accepts_valid_object_nesting() {
    let def = parse(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "address": {
                        "type": "object",
                        "properties": {
                            "street": { "type": "string", "required": true },
                            "geo": {
                                "type": "object",
                                "properties": {
                                    "lat": { "type": "number" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }));
    assert!(
        validate_definition(&def).is_ok(),
        "{:?}",
        validate_definition(&def)
    );
}

#[test]
fn rejects_object_nesting_beyond_max_depth() {
    // Builds a chain of nested objects `object.properties.child` repeated
    // MAX_OBJECT_DEPTH + 1 times, exceeding the allowed nesting depth.
    let mut field = json!({ "type": "string" });
    for _ in 0..=MAX_OBJECT_DEPTH {
        field = json!({
            "type": "object",
            "properties": { "child": field }
        });
    }

    let def = parse(json!({
        "name": "task-management",
        "entity_types": {
            "task": { "fields": { "root": field } }
        }
    }));
    let err = validate_definition(&def).unwrap_err();
    match err {
        YorishiroError::ValidationFailed { details, .. } => {
            assert!(details.iter().any(|d| d.problem.contains("max depth")));
        }
        _ => panic!("expected ValidationFailed"),
    }
}

#[test]
fn rejects_array_items_object_without_properties() {
    let def = parse(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "contacts": { "type": "array", "items": { "type": "object" } }
                }
            }
        }
    }));
    assert!(validate_definition(&def).is_err());
}

#[test]
fn accepts_array_items_object_with_properties() {
    let def = parse(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "contacts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": { "name": { "type": "string", "required": true } }
                        }
                    }
                }
            }
        }
    }));
    assert!(
        validate_definition(&def).is_ok(),
        "{:?}",
        validate_definition(&def)
    );
}
