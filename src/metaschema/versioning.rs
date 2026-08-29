use std::collections::BTreeMap;

use serde::Serialize;

use super::types::{FieldDef, MetaSchemaDefinition};

/// Diff result describing whether a metaschema change is backward compatible.
/// When `is_breaking = true`, the caller must INSERT a new version row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersioningDiff {
    pub is_breaking: bool,
    pub reasons: Vec<String>,
}

/// Non-breaking: adding an optional field, description changes, adding enum values.
/// Breaking: removing/renaming a field, changing its type, making it required, removing an entity_type, changing a relation_type.
pub fn diff(old: &MetaSchemaDefinition, new: &MetaSchemaDefinition) -> VersioningDiff {
    let mut reasons = Vec::new();

    for (type_name, old_entity_type) in &old.entity_types {
        let Some(new_entity_type) = new.entity_types.get(type_name) else {
            reasons.push(format!("entity_type '{type_name}' was removed"));
            continue;
        };

        diff_fields(
            type_name,
            &old_entity_type.fields,
            &new_entity_type.fields,
            &mut reasons,
        );
    }

    for (relation_name, old_relation) in &old.relation_types {
        match new.relation_types.get(relation_name) {
            None => reasons.push(format!("relation_type '{relation_name}' was removed")),
            Some(new_relation) => {
                if old_relation.source != new_relation.source
                    || old_relation.target != new_relation.target
                {
                    reasons.push(format!(
                        "relation_type '{relation_name}' source/target changed"
                    ));
                }
            }
        }
    }

    VersioningDiff {
        is_breaking: !reasons.is_empty(),
        reasons,
    }
}

/// Whether an upper bound got stricter: a lower ceiling, or a ceiling where there was none.
///
/// Comparing the two `Option`s directly does not express this, and gets both directions backwards.
/// `Option`'s ordering places `None` below every `Some`: `Some(10) < None` is false, so adding a ceiling where there was none would read as compatible, while `None < Some(10)` is true, so *removing* one would read as breaking.
fn lowered_ceiling<T: PartialOrd>(old: Option<T>, new: Option<T>) -> bool {
    match (old, new) {
        // A ceiling where there was none: anything above it was valid a moment ago.
        (None, Some(_)) => true,
        // Dropping the ceiling only ever admits more data.
        (Some(_), None) => false,
        (Some(old), Some(new)) => new < old,
        (None, None) => false,
    }
}

/// The floor-side mirror of [`lowered_ceiling`].
/// A bare `new > old` happens to give these two cases the right answer, but only by luck of which way `None` sorts: spelling it out keeps the two sides symmetric and stops the next reader from having to recall that ordering.
fn raised_floor<T: PartialOrd>(old: Option<T>, new: Option<T>) -> bool {
    match (old, new) {
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (Some(old), Some(new)) => new > old,
        (None, None) => false,
    }
}

/// Compares two field maps under a common `path` prefix (e.g. `"task"` at the top level, `"task.address"` when recursing into a nested object), pushing human-readable breaking-change reasons into `reasons`.
fn diff_fields(
    path: &str,
    old_fields: &BTreeMap<String, FieldDef>,
    new_fields: &BTreeMap<String, FieldDef>,
    reasons: &mut Vec<String>,
) {
    for (field_name, old_field) in old_fields {
        let Some(new_field) = new_fields.get(field_name) else {
            reasons.push(format!(
                "field '{field_name}' was removed from '{path}' (or renamed)"
            ));
            continue;
        };

        if old_field.r#type != new_field.r#type {
            reasons.push(format!(
                "field '{path}.{field_name}' changed type from {:?} to {:?}",
                old_field.r#type, new_field.r#type
            ));
        }

        if !old_field.required && new_field.required {
            reasons.push(format!("field '{path}.{field_name}' became required"));
        }

        if let (Some(old_items), Some(new_items)) = (&old_field.items, &new_field.items) {
            if old_items.r#type != new_items.r#type {
                reasons.push(format!(
                    "field '{path}.{field_name}' array items type changed from '{}' to '{}'",
                    old_items.r#type, new_items.r#type
                ));
            } else if old_items.r#type == "object" {
                diff_fields(
                    &format!("{path}.{field_name}[]"),
                    old_items.properties.as_ref().unwrap_or(&BTreeMap::new()),
                    new_items.properties.as_ref().unwrap_or(&BTreeMap::new()),
                    reasons,
                );
            }
        }

        if let (Some(old_properties), Some(new_properties)) =
            (&old_field.properties, &new_field.properties)
        {
            diff_fields(
                &format!("{path}.{field_name}"),
                old_properties,
                new_properties,
                reasons,
            );
        }

        // Removing the enum constraint entirely (Some -> None) is treated as a non-breaking widening.
        // Only the case where the constraint remains but an existing value is no longer allowed is breaking.
        if let Some(old_enum) = &old_field.enum_values
            && let Some(new_enum) = &new_field.enum_values
        {
            for value in old_enum {
                if !new_enum.contains(value) {
                    reasons.push(format!(
                        "field '{path}.{field_name}' enum value '{value}' was removed"
                    ));
                }
            }
        }

        // Narrowing a constraint (adding a new one, or making an existing one stricter) can invalidate data that was valid under the old schema → breaking.
        let fp = |attr: &str| format!("field '{path}.{field_name}' {attr}");

        if old_field.format.is_none() && new_field.format.is_some() {
            reasons.push(fp("added a format constraint"));
        } else if old_field.format != new_field.format
            && old_field.format.is_some()
            && new_field.format.is_some()
        {
            reasons.push(fp("changed its format constraint"));
        }

        if raised_floor(old_field.minimum, new_field.minimum) {
            reasons.push(fp("raised its minimum"));
        }
        if lowered_ceiling(old_field.maximum, new_field.maximum) {
            reasons.push(fp("lowered its maximum"));
        }

        if raised_floor(old_field.min_length, new_field.min_length) {
            reasons.push(fp("raised its minLength"));
        }
        if lowered_ceiling(old_field.max_length, new_field.max_length) {
            reasons.push(fp("lowered its maxLength"));
        }

        if old_field.pattern.is_none() && new_field.pattern.is_some() {
            reasons.push(fp("added a pattern constraint"));
        } else if old_field.pattern != new_field.pattern
            && old_field.pattern.is_some()
            && new_field.pattern.is_some()
        {
            reasons.push(fp("changed its pattern constraint"));
        }

        if raised_floor(old_field.min_items, new_field.min_items) {
            reasons.push(fp("raised its minItems"));
        }
        if lowered_ceiling(old_field.max_items, new_field.max_items) {
            reasons.push(fp("lowered its maxItems"));
        }

        if !old_field.unique_items && new_field.unique_items {
            reasons.push(fp("added uniqueItems constraint"));
        }
    }

    for (field_name, new_field) in new_fields {
        if !old_fields.contains_key(field_name) && new_field.required {
            reasons.push(format!(
                "new field '{path}.{field_name}' was added as required"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
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
    /// A naive comparison therefore misses a ceiling added where there was none, which is exactly what this asserts against.
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
}
