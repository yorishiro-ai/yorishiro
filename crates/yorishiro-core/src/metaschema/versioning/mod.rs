use std::collections::BTreeMap;

use serde::Serialize;
use utoipa::ToSchema;

use super::types::{FieldDef, MetaSchemaDefinition};

/// Diff result describing whether a metaschema change is backward compatible.
/// When `is_breaking = true`, the caller must INSERT a new version row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
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
/// Comparing the two `Option`s directly does not express this.
/// `Option`'s ordering places `None` below every `Some`, so `Some(10) < None` is false, and adding a ceiling where there was none read as compatible; `None < Some(10)` is true, so *removing* one read as breaking.
/// Both backwards.
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
#[path = "../../../tests/metaschema/versioning/mod.rs"]
mod tests;
