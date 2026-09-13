use crate::error::{ValidationDetail, ValidationErrorCode, YorishiroError};

use super::types::{FieldDef, FieldTypeName, MetaSchemaDefinition};

/// Maximum nesting depth for object-type fields, to prevent unbounded recursion from malformed or adversarial schema definitions.
#[doc(hidden)]
pub const MAX_OBJECT_DEPTH: usize = 5;

/// Escapes `~`/`/` per RFC6901 before embedding a value as a JSON Pointer segment.
fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// Validates the metaschema definition itself:
/// - entity_types / relation_types must have minimal internal consistency
/// - relation_types' source/target must reference keys existing in this definition's entity_types
/// - array-type fields only allow items.type == "string" or "object" (with properties for the latter)
/// - object-type fields require non-empty properties, nested up to MAX_OBJECT_DEPTH levels
/// - format is only valid for string fields; minimum/maximum are only valid for number/integer fields and require minimum <= maximum
pub fn validate_definition(def: &MetaSchemaDefinition) -> Result<(), YorishiroError> {
    let mut details = Vec::new();

    if def.name.trim().is_empty() {
        details.push(ValidationDetail {
            field: "/name".into(),
            problem: "name must not be empty".into(),
            code: ValidationErrorCode::EmptyRequired,
            expected: None,
            actual: None,
        });
    }

    if def.entity_types.is_empty() {
        details.push(ValidationDetail {
            field: "/entity_types".into(),
            problem: "at least one entity type is required".into(),
            code: ValidationErrorCode::EmptyRequired,
            expected: Some("1+".into()),
            actual: Some("0".into()),
        });
    }

    for (type_name, entity_type) in &def.entity_types {
        for (field_name, field) in &entity_type.fields {
            let field_path = format!(
                "/entity_types/{}/fields/{}",
                escape_pointer_segment(type_name),
                escape_pointer_segment(field_name)
            );
            validate_field(field, &field_path, 1, &mut details);
        }
    }

    for (relation_name, relation) in &def.relation_types {
        let relation_path = format!("/relation_types/{}", escape_pointer_segment(relation_name));

        if !def.entity_types.contains_key(&relation.source) {
            details.push(ValidationDetail {
                field: format!("{relation_path}/source"),
                problem: format!(
                    "source entity type '{}' is not defined in entity_types",
                    relation.source
                ),
                code: ValidationErrorCode::UndefinedEntityType,
                expected: Some("defined in entity_types".into()),
                actual: Some(relation.source.clone()),
            });
        }

        if !def.entity_types.contains_key(&relation.target) {
            details.push(ValidationDetail {
                field: format!("{relation_path}/target"),
                problem: format!(
                    "target entity type '{}' is not defined in entity_types",
                    relation.target
                ),
                code: ValidationErrorCode::UndefinedEntityType,
                expected: Some("defined in entity_types".into()),
                actual: Some(relation.target.clone()),
            });
        }
    }

    if details.is_empty() {
        Ok(())
    } else {
        Err(YorishiroError::ValidationFailed {
            message: format!("metaschema definition '{}' is invalid", def.name),
            details,
            hint: "Check the consistency between entity_types and relation_types".into(),
        })
    }
}

fn validate_field(
    field: &FieldDef,
    field_path: &str,
    depth: usize,
    details: &mut Vec<ValidationDetail>,
) {
    if field.r#type == FieldTypeName::Array {
        match &field.items {
            Some(items) if items.r#type == "string" => {}
            Some(items) if items.r#type == "object" => match &items.properties {
                Some(properties) if !properties.is_empty() => {
                    if depth >= MAX_OBJECT_DEPTH {
                        details.push(ValidationDetail {
                            field: format!("{field_path}/items/properties"),
                            problem: format!(
                                "object nesting exceeds max depth of {MAX_OBJECT_DEPTH}"
                            ),
                            code: ValidationErrorCode::DepthExceeded,
                            expected: Some(MAX_OBJECT_DEPTH.to_string()),
                            actual: Some(depth.to_string()),
                        });
                    } else {
                        for (child_name, child_field) in properties {
                            let child_path = format!(
                                "{field_path}/items/properties/{}",
                                escape_pointer_segment(child_name)
                            );
                            validate_field(child_field, &child_path, depth + 1, details);
                        }
                    }
                }
                _ => details.push(ValidationDetail {
                    field: format!("{field_path}/items/properties"),
                    problem: "array items.type = 'object' requires non-empty properties".into(),
                    code: ValidationErrorCode::EmptyObjectProperties,
                    expected: Some("non-empty properties".into()),
                    actual: None,
                }),
            },
            Some(items) => details.push(ValidationDetail {
                field: format!("{field_path}/items/type"),
                problem: format!(
                    "array items.type must be 'string' or 'object', got '{}'",
                    items.r#type
                ),
                code: ValidationErrorCode::InvalidArrayItemType,
                expected: Some("'string' or 'object'".into()),
                actual: Some(items.r#type.clone()),
            }),
            None => details.push(ValidationDetail {
                field: format!("{field_path}/items"),
                problem: "array field requires items.type = 'string' or 'object'".into(),
                code: ValidationErrorCode::EmptyRequired,
                expected: Some("items.type".into()),
                actual: None,
            }),
        };
    }

    if field.r#type == FieldTypeName::Object {
        match &field.properties {
            Some(properties) if !properties.is_empty() => {
                if depth >= MAX_OBJECT_DEPTH {
                    details.push(ValidationDetail {
                        field: format!("{field_path}/properties"),
                        problem: format!("object nesting exceeds max depth of {MAX_OBJECT_DEPTH}"),
                        code: ValidationErrorCode::DepthExceeded,
                        expected: Some(MAX_OBJECT_DEPTH.to_string()),
                        actual: Some(depth.to_string()),
                    });
                } else {
                    for (child_name, child_field) in properties {
                        let child_path = format!(
                            "{field_path}/properties/{}",
                            escape_pointer_segment(child_name)
                        );
                        validate_field(child_field, &child_path, depth + 1, details);
                    }
                }
            }
            _ => details.push(ValidationDetail {
                field: field_path.to_string(),
                problem: "object field requires non-empty properties".into(),
                code: ValidationErrorCode::EmptyObjectProperties,
                expected: Some("non-empty properties".into()),
                actual: None,
            }),
        }
    }

    if let Some(format) = &field.format {
        if field.r#type != FieldTypeName::String {
            details.push(ValidationDetail {
                field: format!("{field_path}/format"),
                problem: format!(
                    "format is only valid for string fields, but field type is {:?}",
                    field.r#type
                ),
                code: ValidationErrorCode::TypeMismatch,
                expected: Some("string".into()),
                actual: Some(format!("{:?}", field.r#type)),
            });
        } else if !matches!(
            format.as_str(),
            "date" | "date-time" | "uri" | "email" | "uuid"
        ) {
            details.push(ValidationDetail {
                field: format!("{field_path}/format"),
                problem: format!(
                    "unsupported string format '{format}' (expected date / date-time / uri / email / uuid)"
                ),
                code: ValidationErrorCode::UnsupportedFormat,
                expected: Some("date / date-time / uri / email / uuid".into()),
                actual: Some(format.clone()),
            });
        }
    }

    let numeric_type = matches!(field.r#type, FieldTypeName::Number | FieldTypeName::Integer);
    if !numeric_type && (field.minimum.is_some() || field.maximum.is_some()) {
        details.push(ValidationDetail {
            field: field_path.to_string(),
            problem: format!(
                "minimum/maximum are only valid for number/integer fields, but field type is {:?}",
                field.r#type
            ),
            code: ValidationErrorCode::NumericOnNonNumeric,
            expected: Some("number or integer".into()),
            actual: Some(format!("{:?}", field.r#type)),
        });
    }
    if let (Some(minimum), Some(maximum)) = (field.minimum, field.maximum)
        && minimum > maximum
    {
        details.push(ValidationDetail {
            field: field_path.to_string(),
            problem: format!("minimum ({minimum}) must not exceed maximum ({maximum})"),
            code: ValidationErrorCode::MinExceedsMax,
            expected: format!("<= {maximum}").into(),
            actual: Some(minimum.to_string()),
        });
    }

    let string_type = field.r#type == FieldTypeName::String;
    if !string_type
        && (field.min_length.is_some() || field.max_length.is_some() || field.pattern.is_some())
    {
        details.push(ValidationDetail {
            field: field_path.to_string(),
            problem: format!(
                "minLength/maxLength/pattern are only valid for string fields, but field type is {:?}",
                field.r#type
            ),
            code: ValidationErrorCode::StringConstraintOnNonString,
            expected: Some("string".into()),
            actual: Some(format!("{:?}", field.r#type)),
        });
    }
    if let (Some(min), Some(max)) = (field.min_length, field.max_length)
        && min > max
    {
        details.push(ValidationDetail {
            field: field_path.to_string(),
            problem: format!("minLength ({min}) must not exceed maxLength ({max})"),
            code: ValidationErrorCode::MinExceedsMax,
            expected: format!("<= {max}").into(),
            actual: Some(min.to_string()),
        });
    }
    if let Some(pattern) = &field.pattern
        && regex::Regex::new(pattern).is_err()
    {
        details.push(ValidationDetail {
            field: format!("{field_path}/pattern"),
            problem: format!("invalid regular expression: '{pattern}'"),
            code: ValidationErrorCode::InvalidPattern,
            expected: Some("valid regex".into()),
            actual: Some(pattern.clone()),
        });
    }

    let array_type = field.r#type == FieldTypeName::Array;
    if !array_type && (field.min_items.is_some() || field.max_items.is_some() || field.unique_items)
    {
        details.push(ValidationDetail {
            field: field_path.to_string(),
            problem: format!(
                "minItems/maxItems/uniqueItems are only valid for array fields, but field type is {:?}",
                field.r#type
            ),
            code: ValidationErrorCode::ArrayConstraintOnNonArray,
            expected: Some("array".into()),
            actual: Some(format!("{:?}", field.r#type)),
        });
    }
    if let (Some(min), Some(max)) = (field.min_items, field.max_items)
        && min > max
    {
        details.push(ValidationDetail {
            field: field_path.to_string(),
            problem: format!("minItems ({min}) must not exceed maxItems ({max})"),
            code: ValidationErrorCode::MinExceedsMax,
            expected: format!("<= {max}").into(),
            actual: Some(min.to_string()),
        });
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::metaschema::types::{ArrayItems, EntityTypeDef, RelationTypeDef};
    use proptest::prelude::*;
    use std::collections::BTreeMap;

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
            .prop_map(|(name, description, entity_types, relation_types)| {
                MetaSchemaDefinition {
                    name,
                    description,
                    entity_types,
                    relation_types,
                }
            })
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
}
