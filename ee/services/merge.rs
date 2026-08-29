//! Three-way comparison of metaschema definitions.
//!
//! A workspace's schema is a copy of a template, and both sides can move after the copy is taken.
//! Comparing against the base (the template as it stood when copied), not just upstream vs. local, is what tells an upstream addition apart from a local one: with only two sides both look like "present there, absent here", and following the template would silently delete the workspace's own fields.
//!
//! This module classifies.
//! It does not apply anything: a conflict is a question for a person, and answering it by picking a side would invalidate whichever entities were written against the losing definition.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::error::YorishiroError;
use crate::metaschema::{EntityTypeDef, FieldDef, MetaSchemaDefinition};

/// What should happen to one field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeVerdict {
    /// Upstream added it and the workspace has nothing by that name.
    /// Safe to take: it is new structure, and adding an optional field invalidates nothing already stored.
    AutoAdd,
    /// Upstream changed it and the workspace did not.
    /// Taking the change loses no local work, since there is none to lose.
    AutoUpdate,
    /// The workspace's own, unknown upstream.
    /// Kept: following a template must not delete what the workspace added on top of it.
    KeepLocal,
    /// Both sides changed it, differently.
    /// Nothing here decides which is right; a person does.
    Conflict,
}

/// One field's classification.
#[derive(Debug, Clone, Serialize)]
pub struct FieldMerge {
    pub entity_type: String,
    pub field: String,
    pub verdict: MergeVerdict,
    /// What differs, in the terms the operator will judge it by.
    pub detail: String,
}

/// The classification of every field that is not identical across the three.
#[derive(Debug, Clone, Serialize)]
pub struct MergePlan {
    pub fields: Vec<FieldMerge>,
}

impl MergePlan {
    /// Whether anything needs a person.
    /// A plan with no conflicts can be applied whole; one with any cannot be applied at all, since a partial merge would leave the schema in a state neither side asked for.
    pub fn has_conflicts(&self) -> bool {
        self.fields
            .iter()
            .any(|f| f.verdict == MergeVerdict::Conflict)
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &FieldMerge> {
        self.fields
            .iter()
            .filter(|f| f.verdict == MergeVerdict::Conflict)
    }
}

/// Compares three definitions field by field.
///
/// `base` is the template as copied, `upstream` the template now, `local` the workspace's own.
/// Fields identical in all three are omitted: a plan lists what to decide, not what exists.
pub fn three_way(
    base: &MetaSchemaDefinition,
    upstream: &MetaSchemaDefinition,
    local: &MetaSchemaDefinition,
) -> MergePlan {
    let mut fields = Vec::new();

    // Every entity type named by any of the three.
    // A type only upstream is as much a difference as a field only upstream.
    let entity_types: BTreeSet<&String> = base
        .entity_types
        .keys()
        .chain(upstream.entity_types.keys())
        .chain(local.entity_types.keys())
        .collect();

    for entity_type in entity_types {
        let base_fields = base.entity_types.get(entity_type).map(|t| &t.fields);
        let upstream_fields = upstream.entity_types.get(entity_type).map(|t| &t.fields);
        let local_fields = local.entity_types.get(entity_type).map(|t| &t.fields);

        let names: BTreeSet<&String> = base_fields
            .into_iter()
            .flat_map(|f| f.keys())
            .chain(upstream_fields.into_iter().flat_map(|f| f.keys()))
            .chain(local_fields.into_iter().flat_map(|f| f.keys()))
            .collect();

        for name in names {
            let in_base = base_fields.and_then(|f| f.get(name));
            let in_upstream = upstream_fields.and_then(|f| f.get(name));
            let in_local = local_fields.and_then(|f| f.get(name));

            if let Some((verdict, detail)) = classify(in_base, in_upstream, in_local) {
                fields.push(FieldMerge {
                    entity_type: entity_type.clone(),
                    field: name.clone(),
                    verdict,
                    detail,
                });
            }
        }
    }

    MergePlan { fields }
}

/// One field's verdict, or `None` when the three agree and there is nothing to decide.
fn classify(
    base: Option<&FieldDef>,
    upstream: Option<&FieldDef>,
    local: Option<&FieldDef>,
) -> Option<(MergeVerdict, String)> {
    let upstream_moved = !same(base, upstream);
    let local_moved = !same(base, local);

    match (upstream_moved, local_moved) {
        // Neither side moved, or both moved the same way.
        // Nothing to decide either way: if they agree, the answer is already what both want.
        (false, false) => None,
        (true, false) => {
            // Only upstream moved.
            // Adding is distinguishable from changing, and the operator reads them differently even though both are taken automatically.
            if base.is_none() {
                Some((
                    MergeVerdict::AutoAdd,
                    "added upstream; absent here".to_string(),
                ))
            } else if upstream.is_none() {
                Some((
                    MergeVerdict::AutoUpdate,
                    "removed upstream; unchanged here".to_string(),
                ))
            } else {
                Some((
                    MergeVerdict::AutoUpdate,
                    "changed upstream; unchanged here".to_string(),
                ))
            }
        }
        (false, true) => {
            if base.is_none() {
                Some((
                    MergeVerdict::KeepLocal,
                    "added here; unknown upstream".to_string(),
                ))
            } else {
                Some((
                    MergeVerdict::KeepLocal,
                    "changed here; unchanged upstream".to_string(),
                ))
            }
        }
        (true, true) => {
            // Both moved.
            // Identical moves are not a conflict: two people adding the same field with the same type have agreed, not disagreed.
            if same(upstream, local) {
                None
            } else {
                Some((MergeVerdict::Conflict, describe_conflict(upstream, local)))
            }
        }
    }
}

/// Whether two optional field definitions are the same.
/// Compared by their serialised form: `FieldDef` carries unknown `x-` attributes in a flattened map, and a comparison that only looked at the named fields would call two definitions equal while an extension differed.
fn same(a: Option<&FieldDef>, b: Option<&FieldDef>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => match (serde_json::to_value(a), serde_json::to_value(b)) {
            (Ok(a), Ok(b)) => a == b,
            // Not `.ok() == .ok()`, which maps two *failures* to `None == None` and calls them equal, and "equal" here means the field never enters the plan, so a genuine upstream change would be neither reported nor applied.
            _ => false,
        },
        _ => false,
    }
}

fn describe_conflict(upstream: Option<&FieldDef>, local: Option<&FieldDef>) -> String {
    match (upstream, local) {
        (Some(u), Some(l)) if u.r#type != l.r#type => format!(
            "type differs: upstream '{}', here '{}'",
            type_name(u),
            type_name(l)
        ),
        (Some(_), Some(_)) => "both changed, differently".to_string(),
        (None, Some(_)) => "removed upstream, changed here".to_string(),
        (Some(_), None) => "changed upstream, removed here".to_string(),
        (None, None) => unreachable!("classify only reaches here when the two differ"),
    }
}

fn type_name(field: &FieldDef) -> String {
    serde_json::to_value(field.r#type)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Produces the definition a plan describes: upstream's version of everything it changed alone, the workspace's version of everything it changed alone.
///
/// Refuses a plan with conflicts.
/// There is no answer to apply for those (that is what a conflict means), and applying the rest would leave a definition that is neither what the merge produced nor what was there before, with no record of which fields were skipped.
///
/// The result is a definition, not a stored schema.
/// Whether writing it mints a new version is a separate decision, and one this function deliberately does not make.
pub fn apply_plan(
    plan: &MergePlan,
    upstream: &MetaSchemaDefinition,
    local: &MetaSchemaDefinition,
) -> Result<MetaSchemaDefinition, YorishiroError> {
    if plan.has_conflicts() {
        let fields: Vec<String> = plan
            .conflicts()
            .map(|f| format!("{}.{}", f.entity_type, f.field))
            .collect();
        return Err(YorishiroError::ValidationFailed {
            message: format!("the merge has {} unresolved conflict(s)", fields.len()),
            details: plan
                .conflicts()
                .map(|f| crate::error::ValidationDetail {
                    field: format!("/{}/{}", f.entity_type, f.field),
                    problem: f.detail.clone(),
                })
                .collect(),
            hint: format!(
                "resolve {} before applying; a partially applied merge is a definition neither \
                 side asked for",
                fields.join(", ")
            ),
        });
    }

    // Start from the workspace's own definition: everything it holds stays unless the plan says upstream changed that field alone.
    // Starting from upstream instead would silently drop every local addition, which is the failure the base exists to prevent.
    let mut merged = local.clone();

    for field in &plan.fields {
        match field.verdict {
            MergeVerdict::AutoAdd | MergeVerdict::AutoUpdate => {
                let upstream_field = upstream
                    .entity_types
                    .get(&field.entity_type)
                    .and_then(|t| t.fields.get(&field.field));

                match upstream_field {
                    Some(def) => {
                        // The entity type may not exist locally yet: an upstream addition of a whole type arrives field by field.
                        merged
                            .entity_types
                            .entry(field.entity_type.clone())
                            .or_insert_with(|| {
                                upstream
                                    .entity_types
                                    .get(&field.entity_type)
                                    .map(|t| EntityTypeDef {
                                        description: t.description.clone(),
                                        fields: Default::default(),
                                    })
                                    .unwrap_or_else(|| EntityTypeDef {
                                        description: None,
                                        fields: Default::default(),
                                    })
                            })
                            .fields
                            .insert(field.field.clone(), def.clone());
                    }
                    None => {
                        // Removed upstream, untouched locally: the plan calls that an update, and the update is the removal.
                        if let Some(entity_type) = merged.entity_types.get_mut(&field.entity_type) {
                            entity_type.fields.remove(&field.field);
                        }
                    }
                }
            }
            // Kept as it already is in `local`, which is what `merged` started from.
            MergeVerdict::KeepLocal => {}
            MergeVerdict::Conflict => unreachable!("refused above"),
        }
    }

    Ok(merged)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Builds a definition with one entity type whose fields are given as JSON.
    fn def(fields: serde_json::Value) -> MetaSchemaDefinition {
        serde_json::from_value(json!({
            "name": "task-management",
            "entity_types": { "task": { "fields": fields } }
        }))
        .unwrap()
    }

    fn verdict_for(plan: &MergePlan, field: &str) -> Option<MergeVerdict> {
        plan.fields
            .iter()
            .find(|f| f.field == field)
            .map(|f| f.verdict)
    }

    /// Upstream added a field the workspace has never had.
    /// Taking it adds structure without touching anything stored.
    #[test]
    fn a_field_only_upstream_added_is_taken() {
        let base = def(json!({ "title": { "type": "string" } }));
        let upstream = def(json!({
            "title": { "type": "string" },
            "category": { "type": "string" }
        }));
        let local = base.clone();

        let plan = three_way(&base, &upstream, &local);

        assert_eq!(verdict_for(&plan, "category"), Some(MergeVerdict::AutoAdd));
        assert_eq!(
            verdict_for(&plan, "title"),
            None,
            "an untouched field is not listed"
        );
    }

    /// The workspace's own field.
    /// Following the template must not delete it: "absent upstream" and "removed upstream" must not be treated the same.
    #[test]
    fn a_field_only_the_workspace_added_is_kept() {
        let base = def(json!({ "title": { "type": "string" } }));
        let upstream = base.clone();
        let local = def(json!({
            "title": { "type": "string" },
            "internal_ref": { "type": "string" }
        }));

        let plan = three_way(&base, &upstream, &local);

        assert_eq!(
            verdict_for(&plan, "internal_ref"),
            Some(MergeVerdict::KeepLocal)
        );
    }

    /// Upstream changed a field nobody here touched.
    /// No local work to lose.
    #[test]
    fn an_upstream_change_to_an_untouched_field_is_taken() {
        let base = def(json!({ "title": { "type": "string" } }));
        let upstream = def(json!({ "title": { "type": "string", "maxLength": 200 } }));
        let local = base.clone();

        let plan = three_way(&base, &upstream, &local);

        assert_eq!(verdict_for(&plan, "title"), Some(MergeVerdict::AutoUpdate));
    }

    /// Both sides changed the same field differently.
    /// Nothing here picks a side: whichever lost would leave the entities written against it failing validation.
    #[test]
    fn a_field_both_sides_changed_is_a_conflict() {
        let base = def(json!({ "priority": { "type": "string" } }));
        let upstream = def(json!({ "priority": { "type": "integer" } }));
        let local = def(json!({ "priority": { "type": "boolean" } }));

        let plan = three_way(&base, &upstream, &local);

        assert_eq!(verdict_for(&plan, "priority"), Some(MergeVerdict::Conflict));
        assert!(plan.has_conflicts());
        let conflict = plan.conflicts().next().unwrap();
        assert!(
            conflict.detail.contains("integer") && conflict.detail.contains("boolean"),
            "the operator has to see both sides: {}",
            conflict.detail
        );
    }

    /// Both added the same field the same way.
    /// That is agreement, not a conflict: reporting it would make an operator adjudicate a decision already shared.
    #[test]
    fn the_same_change_on_both_sides_is_not_a_conflict() {
        let base = def(json!({ "title": { "type": "string" } }));
        let added = def(json!({
            "title": { "type": "string" },
            "due": { "type": "string", "format": "date" }
        }));

        let plan = three_way(&base, &added, &added);

        assert!(!plan.has_conflicts());
        assert_eq!(verdict_for(&plan, "due"), None);
    }

    /// An entity type present only upstream is as much a difference as a field, and each of its fields is reported.
    #[test]
    fn a_new_entity_type_upstream_is_reported_field_by_field() {
        let base = def(json!({ "title": { "type": "string" } }));
        let upstream: MetaSchemaDefinition = serde_json::from_value(json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "title": { "type": "string" } } },
                "project": { "fields": { "name": { "type": "string" } } }
            }
        }))
        .unwrap();
        let local = base.clone();

        let plan = three_way(&base, &upstream, &local);

        let project = plan
            .fields
            .iter()
            .find(|f| f.entity_type == "project")
            .expect("a type only upstream is a difference");
        assert_eq!(project.verdict, MergeVerdict::AutoAdd);
    }

    /// Fields differing only in an unknown `x-` extension still differ.
    /// `FieldDef` keeps those in a flattened map, so a comparison over the named fields alone would call these equal.
    #[test]
    fn an_extension_attribute_is_part_of_the_comparison() {
        let base = def(json!({ "title": { "type": "string" } }));
        let upstream =
            def(json!({ "title": { "type": "string", "x-ui": { "widget": "textarea" } } }));
        let local = base.clone();

        let plan = three_way(&base, &upstream, &local);

        assert_eq!(verdict_for(&plan, "title"), Some(MergeVerdict::AutoUpdate));
    }

    /// A plan with nothing to decide is empty rather than a list of agreements.
    #[test]
    fn identical_definitions_produce_an_empty_plan() {
        let d = def(json!({ "title": { "type": "string" } }));
        let plan = three_way(&d, &d, &d);
        assert!(plan.fields.is_empty());
        assert!(!plan.has_conflicts());
    }

    /// Upstream's addition arrives and the workspace's own survives, in the same merge.
    #[test]
    fn applying_takes_upstream_additions_and_keeps_local_ones() {
        let base = def(json!({ "title": { "type": "string" } }));
        let upstream = def(json!({
            "title": { "type": "string" },
            "category": { "type": "string" }
        }));
        let local = def(json!({
            "title": { "type": "string" },
            "internal_ref": { "type": "string" }
        }));

        let plan = three_way(&base, &upstream, &local);
        let merged = apply_plan(&plan, &upstream, &local).unwrap();

        let fields = &merged.entity_types["task"].fields;
        assert!(
            fields.contains_key("category"),
            "upstream's addition arrives"
        );
        assert!(
            fields.contains_key("internal_ref"),
            "the workspace's own survives"
        );
        assert!(fields.contains_key("title"));
    }

    /// An upstream change to an untouched field is taken.
    #[test]
    fn applying_takes_an_upstream_change_to_an_untouched_field() {
        let base = def(json!({ "title": { "type": "string" } }));
        let upstream = def(json!({ "title": { "type": "string", "maxLength": 200 } }));
        let local = base.clone();

        let plan = three_way(&base, &upstream, &local);
        let merged = apply_plan(&plan, &upstream, &local).unwrap();

        assert_eq!(
            merged.entity_types["task"].fields["title"].max_length,
            Some(200)
        );
    }

    /// A conflicting plan is refused whole.
    /// Applying the rest would leave a definition neither side asked for, with nothing recording which fields were skipped.
    #[test]
    fn applying_refuses_a_plan_with_conflicts() {
        let base = def(json!({ "priority": { "type": "string" } }));
        let upstream = def(json!({
            "priority": { "type": "integer" },
            "category": { "type": "string" }
        }));
        let local = def(json!({ "priority": { "type": "boolean" } }));

        let plan = three_way(&base, &upstream, &local);
        let err = apply_plan(&plan, &upstream, &local).unwrap_err();

        match err {
            YorishiroError::ValidationFailed { details, .. } => {
                assert!(
                    details.iter().any(|d| d.field.contains("priority")),
                    "the conflicting field is named: {details:?}"
                );
            }
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
    }

    /// A whole entity type added upstream arrives, even though the workspace has no such type to merge into.
    #[test]
    fn applying_adds_an_entity_type_the_workspace_does_not_have() {
        let base = def(json!({ "title": { "type": "string" } }));
        let upstream: MetaSchemaDefinition = serde_json::from_value(json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "title": { "type": "string" } } },
                "project": { "fields": { "name": { "type": "string" } } }
            }
        }))
        .unwrap();
        let local = base.clone();

        let plan = three_way(&base, &upstream, &local);
        let merged = apply_plan(&plan, &upstream, &local).unwrap();

        assert!(
            merged.entity_types.contains_key("project"),
            "a type only upstream is created"
        );
        assert!(merged.entity_types["project"].fields.contains_key("name"));
    }

    /// Nothing to merge produces the workspace's definition unchanged, rather than a rebuild of it.
    #[test]
    fn applying_an_empty_plan_changes_nothing() {
        let d = def(json!({ "title": { "type": "string" } }));
        let plan = three_way(&d, &d, &d);
        let merged = apply_plan(&plan, &d, &d).unwrap();
        assert_eq!(
            serde_json::to_value(&merged).unwrap(),
            serde_json::to_value(&d).unwrap()
        );
    }

    /// Only the workspace changed the field's type (base and upstream agree), so the workspace's own definition is kept rather than reported as a conflict.
    /// A conflict needs *both* sides to have moved, which `a_field_both_sides_changed_is_a_conflict` covers.
    #[test]
    fn a_type_changed_only_locally_is_kept() {
        let upstream: MetaSchemaDefinition = serde_json::from_value(json!({
            "name": "t",
            "entity_types": { "task": { "fields": { "title": { "type": "string" } } } }
        }))
        .unwrap();
        let local: MetaSchemaDefinition = serde_json::from_value(json!({
            "name": "t",
            "entity_types": { "task": { "fields": { "title": { "type": "integer" } } } }
        }))
        .unwrap();

        let plan = three_way(&upstream, &upstream, &local);
        assert_eq!(
            verdict_for(&plan, "title"),
            Some(MergeVerdict::KeepLocal),
            "only the workspace moved, so its own definition is kept"
        );
    }
}
