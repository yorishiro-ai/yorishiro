/// Tests for metaschema definition validation.
use proptest::prelude::*;
use serde_json::json;
use std::collections::BTreeMap;
use yorishiro::error::YorishiroError;
use yorishiro::metaschema::{
    ArrayItems, EntityTypeDef, FieldDef, FieldTypeName, MAX_OBJECT_DEPTH, MetaSchemaDefinition,
    RelationTypeDef, validate_definition,
};

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
    // Builds a chain of nested objects `object.properties.child` repeated MAX_OBJECT_DEPTH + 1 times, exceeding the allowed nesting depth.
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

fn short_string() -> impl Strategy<Value = String> {
    "[ -~]{0,16}"
}

fn any_field_type() -> impl Strategy<Value = FieldTypeName> {
    prop_oneof![
        Just(FieldTypeName::String),
        Just(FieldTypeName::Number),
        Just(FieldTypeName::Integer),
        Just(FieldTypeName::Boolean),
        Just(FieldTypeName::Array),
        Just(FieldTypeName::Object),
    ]
}

fn any_field() -> impl Strategy<Value = FieldDef> {
    (
        any_field_type(),
        (any::<bool>(), any::<bool>()),
        prop::option::of(short_string()),
        prop::option::of(proptest::collection::vec(short_string(), 0..4)),
        prop::option::of(short_string()),
        prop::option::of(any::<f64>()),
        prop::option::of(any::<f64>()),
        prop::option::of(any::<u64>()),
        prop::option::of(any::<u64>()),
        prop::option::of(short_string()),
        prop::option::of(any::<u64>()),
        prop::option::of(any::<u64>()),
    )
        .prop_map(
            |(
                r#type,
                (required, unique_items),
                description,
                enum_values,
                format,
                minimum,
                maximum,
                min_length,
                max_length,
                pattern,
                min_items,
                max_items,
            )| FieldDef {
                r#type,
                required,
                description,
                enum_values,
                format,
                minimum,
                maximum,
                min_length,
                max_length,
                pattern,
                min_items,
                max_items,
                unique_items,
                default: None,
                items: None,
                properties: None,
                x_embed: false,
                x_ui: None,
                extra: serde_json::Map::new(),
            },
        )
        .prop_recursive(4, 64, 4, |child| {
            let child = child.boxed();
            (
                prop::option::of(
                    (
                        short_string(),
                        prop::option::of(proptest::collection::btree_map(
                            short_string(),
                            child.clone(),
                            0..4,
                        )),
                    )
                        .prop_map(|(r#type, properties)| ArrayItems { r#type, properties }),
                ),
                prop::option::of(proptest::collection::btree_map(short_string(), child, 0..4)),
            )
                .prop_map(|(items, properties)| {
                    let mut field = FieldDef {
                        r#type: if items.is_some() {
                            FieldTypeName::Array
                        } else if properties.is_some() {
                            FieldTypeName::Object
                        } else {
                            FieldTypeName::String
                        },
                        required: false,
                        description: None,
                        enum_values: None,
                        format: None,
                        minimum: None,
                        maximum: None,
                        min_length: None,
                        max_length: None,
                        pattern: None,
                        min_items: None,
                        max_items: None,
                        unique_items: false,
                        default: None,
                        items: None,
                        properties: None,
                        x_embed: false,
                        x_ui: None,
                        extra: serde_json::Map::new(),
                    };
                    field.items = items;
                    field.properties = properties;
                    field
                })
        })
}

fn any_entity_type() -> impl Strategy<Value = EntityTypeDef> {
    proptest::collection::btree_map(short_string(), any_field(), 0..4).prop_map(|fields| {
        EntityTypeDef {
            description: None,
            fields,
        }
    })
}

fn any_schema_def() -> impl Strategy<Value = MetaSchemaDefinition> {
    (
        short_string(),
        prop::option::of(short_string()),
        proptest::collection::btree_map(short_string(), any_entity_type(), 0..3),
        proptest::collection::btree_map(
            short_string(),
            (short_string(), short_string()).prop_map(|(source, target)| RelationTypeDef {
                source,
                target,
                description: None,
            }),
            0..3,
        ),
    )
        .prop_map(
            |(name, description, entity_types, relation_types)| MetaSchemaDefinition {
                name,
                description,
                entity_types,
                relation_types,
            },
        )
}

proptest! {
    #[test]
    fn validate_definition_never_panics(def in any_schema_def()) {
        let result = std::panic::catch_unwind(|| validate_definition(&def));
        prop_assert!(result.is_ok());
    }

    #[test]
    fn empty_entity_types_are_always_rejected(
        name in short_string(),
        description in prop::option::of(short_string()),
        relation_types in proptest::collection::btree_map(
            short_string(),
            (short_string(), short_string()).prop_map(|(source, target)| RelationTypeDef {
                source,
                target,
                description: None,
            }),
            0..3,
        ),
    ) {
        let definition = MetaSchemaDefinition {
            name,
            description,
            entity_types: BTreeMap::new(),
            relation_types,
        };
        prop_assert!(validate_definition(&definition).is_err());
    }
}
