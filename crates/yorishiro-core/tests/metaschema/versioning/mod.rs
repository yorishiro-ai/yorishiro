use crate::metaschema::{MetaSchemaDefinition, diff};
use serde_json::json;

fn parse(value: serde_json::Value) -> MetaSchemaDefinition {
    serde_json::from_value(value).expect("valid metaschema json")
}

#[test]
fn adding_optional_field_is_non_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": { "title": { "type": "string", "required": true } } } }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {
            "title": { "type": "string", "required": true },
            "note": { "type": "string" }
        } } }
    }));
    let d = diff(&old, &new);
    assert!(!d.is_breaking, "reasons: {:?}", d.reasons);
}

#[test]
fn adding_enum_value_is_non_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {
            "status": { "type": "string", "enum": ["active"] }
        } } }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {
            "status": { "type": "string", "enum": ["active", "done"] }
        } } }
    }));
    let d = diff(&old, &new);
    assert!(!d.is_breaking, "reasons: {:?}", d.reasons);
}

#[test]
fn removing_field_is_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {
            "title": { "type": "string" }, "note": { "type": "string" }
        } } }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": { "title": { "type": "string" } } } }
    }));
    let d = diff(&old, &new);
    assert!(d.is_breaking);
}

#[test]
fn making_field_required_is_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": { "title": { "type": "string" } } } }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": { "title": { "type": "string", "required": true } } } }
    }));
    let d = diff(&old, &new);
    assert!(d.is_breaking);
}

#[test]
fn changing_field_type_is_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": { "done": { "type": "boolean" } } } }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": { "done": { "type": "string" } } } }
    }));
    let d = diff(&old, &new);
    assert!(d.is_breaking);
}

#[test]
fn removing_entity_type_is_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": {
            "task": { "fields": { "title": { "type": "string" } } },
            "project": { "fields": { "title": { "type": "string" } } }
        }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": { "title": { "type": "string" } } } }
    }));
    let d = diff(&old, &new);
    assert!(d.is_breaking);
}

#[test]
fn changing_relation_type_target_is_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": {
            "task": { "fields": {} }, "project": { "fields": {} }, "epic": { "fields": {} }
        },
        "relation_types": { "belongs_to": { "source": "task", "target": "project" } }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": {
            "task": { "fields": {} }, "project": { "fields": {} }, "epic": { "fields": {} }
        },
        "relation_types": { "belongs_to": { "source": "task", "target": "epic" } }
    }));
    let d = diff(&old, &new);
    assert!(d.is_breaking);
}

#[test]
fn adding_new_entity_type_and_relation_type_is_non_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {} } }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": {
            "task": { "fields": {} },
            "project": { "fields": {} }
        },
        "relation_types": { "belongs_to": { "source": "task", "target": "project" } }
    }));
    let d = diff(&old, &new);
    assert!(!d.is_breaking, "reasons: {:?}", d.reasons);
}

#[test]
fn adding_new_required_field_is_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": { "title": { "type": "string" } } } }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {
            "title": { "type": "string" },
            "priority": { "type": "integer", "required": true }
        } } }
    }));
    let d = diff(&old, &new);
    assert!(d.is_breaking, "reasons: {:?}", d.reasons);
}

#[test]
fn renaming_relation_type_is_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {} }, "project": { "fields": {} } },
        "relation_types": { "belongs_to": { "source": "task", "target": "project" } }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {} }, "project": { "fields": {} } },
        "relation_types": { "part_of": { "source": "task", "target": "project" } }
    }));
    let d = diff(&old, &new);
    assert!(d.is_breaking, "reasons: {:?}", d.reasons);
}

#[test]
fn removing_enum_constraint_entirely_is_non_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {
            "status": { "type": "string", "enum": ["active", "done"] }
        } } }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {
            "status": { "type": "string" }
        } } }
    }));
    let d = diff(&old, &new);
    assert!(!d.is_breaking, "reasons: {:?}", d.reasons);
}

#[test]
fn removing_nested_object_field_is_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {
            "address": {
                "type": "object",
                "properties": {
                    "street": { "type": "string" },
                    "city": { "type": "string" }
                }
            }
        } } }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {
            "address": {
                "type": "object",
                "properties": {
                    "street": { "type": "string" }
                }
            }
        } } }
    }));
    let d = diff(&old, &new);
    assert!(d.is_breaking, "reasons: {:?}", d.reasons);
}

#[test]
fn adding_required_nested_object_field_is_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {
            "address": {
                "type": "object",
                "properties": {
                    "street": { "type": "string" }
                }
            }
        } } }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {
            "address": {
                "type": "object",
                "properties": {
                    "street": { "type": "string" },
                    "postal_code": { "type": "string", "required": true }
                }
            }
        } } }
    }));
    let d = diff(&old, &new);
    assert!(d.is_breaking, "reasons: {:?}", d.reasons);
}

#[test]
fn adding_optional_nested_object_field_is_non_breaking() {
    let old = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {
            "address": {
                "type": "object",
                "properties": {
                    "street": { "type": "string" }
                }
            }
        } } }
    }));
    let new = parse(json!({
        "name": "n",
        "entity_types": { "task": { "fields": {
            "address": {
                "type": "object",
                "properties": {
                    "street": { "type": "string" },
                    "postal_code": { "type": "string" }
                }
            }
        } } }
    }));
    let d = diff(&old, &new);
    assert!(!d.is_breaking, "reasons: {:?}", d.reasons);
}

#[test]
fn description_only_change_is_non_breaking() {
    let old = parse(json!({
        "name": "n",
        "description": "old description",
        "entity_types": { "task": { "description": "old", "fields": { "title": { "type": "string" } } } }
    }));
    let new = parse(json!({
        "name": "n",
        "description": "new description",
        "entity_types": { "task": { "description": "new", "fields": { "title": { "type": "string" } } } }
    }));
    let d = diff(&old, &new);
    assert!(!d.is_breaking, "reasons: {:?}", d.reasons);
}

/// Builds a two-version pair of the same single field, so a test only has to state the two attribute sets it cares about.
fn diff_field(
    old_attrs: serde_json::Value,
    new_attrs: serde_json::Value,
) -> crate::metaschema::VersioningDiff {
    let build = |attrs: serde_json::Value| {
        let mut field = json!({ "type": "string" });
        let serde_json::Value::Object(extra) = attrs else {
            panic!("field attributes must be a JSON object");
        };
        field.as_object_mut().unwrap().extend(extra);
        parse(json!({
            "name": "n",
            "entity_types": { "task": { "fields": { "f": field } } }
        }))
    };
    diff(&build(old_attrs), &build(new_attrs))
}

/// Every constraint that can invalidate data which was valid a moment ago.
/// A schema author who tightens one of these and is told the change is compatible will overwrite the live version in place, and existing records stop validating with no new version to roll back to.
///
/// `maximum`/`maxLength`/`maxItems` are here for a reason that the "raised its minimum" cases do not share: those three compare `new < old`, and `Option`'s ordering puts `None` *below* every `Some`, so `Some(10) < None` is false.
/// Adding a ceiling where there was none went undetected.
#[test]
fn tightening_a_constraint_is_breaking() {
    for (label, old_attrs, new_attrs) in [
        ("added a format", json!({}), json!({ "format": "email" })),
        (
            "changed the format",
            json!({ "format": "email" }),
            json!({ "format": "uri" }),
        ),
        ("added a minimum", json!({}), json!({ "minimum": 1 })),
        (
            "raised the minimum",
            json!({ "minimum": 1 }),
            json!({ "minimum": 5 }),
        ),
        ("added a maximum", json!({}), json!({ "maximum": 10 })),
        (
            "lowered the maximum",
            json!({ "maximum": 10 }),
            json!({ "maximum": 5 }),
        ),
        ("added a minLength", json!({}), json!({ "minLength": 1 })),
        (
            "raised the minLength",
            json!({ "minLength": 1 }),
            json!({ "minLength": 5 }),
        ),
        ("added a maxLength", json!({}), json!({ "maxLength": 10 })),
        (
            "lowered the maxLength",
            json!({ "maxLength": 10 }),
            json!({ "maxLength": 5 }),
        ),
        ("added a pattern", json!({}), json!({ "pattern": "^a" })),
        (
            "changed the pattern",
            json!({ "pattern": "^a" }),
            json!({ "pattern": "^b" }),
        ),
        ("added a minItems", json!({}), json!({ "minItems": 1 })),
        (
            "raised the minItems",
            json!({ "minItems": 1 }),
            json!({ "minItems": 5 }),
        ),
        ("added a maxItems", json!({}), json!({ "maxItems": 10 })),
        (
            "lowered the maxItems",
            json!({ "maxItems": 10 }),
            json!({ "maxItems": 5 }),
        ),
        (
            "added uniqueItems",
            json!({}),
            json!({ "uniqueItems": true }),
        ),
    ] {
        let d = diff_field(old_attrs, new_attrs);
        assert!(
            d.is_breaking,
            "{label} must be breaking, but diff reported none"
        );
        assert!(
            d.reasons.iter().any(|r| r.contains("task.f")),
            "{label}: the reason should name the field: {:?}",
            d.reasons
        );
    }
}

/// The mirror image.
/// Widening a constraint can only make previously-invalid data valid, so it must not force a new version: otherwise every relaxation costs a migration for nothing.
#[test]
fn loosening_a_constraint_is_non_breaking() {
    for (label, old_attrs, new_attrs) in [
        (
            "dropped the format",
            json!({ "format": "email" }),
            json!({}),
        ),
        ("dropped the minimum", json!({ "minimum": 5 }), json!({})),
        (
            "lowered the minimum",
            json!({ "minimum": 5 }),
            json!({ "minimum": 1 }),
        ),
        ("dropped the maximum", json!({ "maximum": 5 }), json!({})),
        (
            "raised the maximum",
            json!({ "maximum": 5 }),
            json!({ "maximum": 10 }),
        ),
        (
            "dropped the minLength",
            json!({ "minLength": 5 }),
            json!({}),
        ),
        (
            "dropped the maxLength",
            json!({ "maxLength": 5 }),
            json!({}),
        ),
        (
            "raised the maxLength",
            json!({ "maxLength": 5 }),
            json!({ "maxLength": 10 }),
        ),
        ("dropped the pattern", json!({ "pattern": "^a" }), json!({})),
        ("dropped the minItems", json!({ "minItems": 5 }), json!({})),
        ("dropped the maxItems", json!({ "maxItems": 5 }), json!({})),
        (
            "dropped uniqueItems",
            json!({ "uniqueItems": true }),
            json!({}),
        ),
    ] {
        let d = diff_field(old_attrs, new_attrs);
        assert!(
            !d.is_breaking,
            "{label} must be non-breaking, but diff said: {:?}",
            d.reasons
        );
    }
}
