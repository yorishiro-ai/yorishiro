//! Three-way comparison of metaschema definitions.
//!
//! A workspace's schema is a copy of a template, and both sides can move after the copy is
//! taken. Deciding what to do about that needs three definitions, not two: the template as it
//! stood when copied (base), the template now (upstream), and the workspace's own (local).
//!
//! With only two, an upstream addition and a local one look identical (both are "present
//! there, absent here"), and following the template would silently delete the workspace's own
//! fields. The base is what tells them apart.
//!
//! This module classifies. It does not apply anything: a conflict is a question for a person,
//! and answering it by picking a side would invalidate whichever entities were written against
//! the losing definition.

use std::collections::BTreeSet;

use serde::Serialize;
use utoipa::ToSchema;

use yorishiro_core::error::YorishiroError;
use yorishiro_core::metaschema::{EntityTypeDef, FieldDef, MetaSchemaDefinition};

/// What should happen to one field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeVerdict {
    /// Upstream added it and the workspace has nothing by that name. Safe to take: it is new
    /// structure, and adding an optional field invalidates nothing already stored.
    AutoAdd,
    /// Upstream changed it and the workspace did not. Taking the change loses no local work,
    /// since there is none to lose.
    AutoUpdate,
    /// The workspace's own, unknown upstream. Kept: following a template must not delete what
    /// the workspace added on top of it.
    KeepLocal,
    /// Both sides changed it, differently. Nothing here decides which is right; a person does.
    Conflict,
}

/// One field's classification.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FieldMerge {
    pub entity_type: String,
    pub field: String,
    pub verdict: MergeVerdict,
    /// What differs, in the terms the operator will judge it by.
    pub detail: String,
}

/// The classification of every field that is not identical across the three.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MergePlan {
    pub fields: Vec<FieldMerge>,
}

impl MergePlan {
    /// Whether anything needs a person. A plan with no conflicts can be applied whole; one
    /// with any cannot be applied at all, since a partial merge would leave the schema in a
    /// state neither side asked for.
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

    // Every entity type named by any of the three. A type only upstream is as much a
    // difference as a field only upstream.
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
        // Neither side moved, or both moved the same way. Nothing to decide either way: if
        // they agree, the answer is already what both want.
        (false, false) => None,
        (true, false) => {
            // Only upstream moved. Adding is distinguishable from changing, and the operator
            // reads them differently even though both are taken automatically.
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
            // Both moved. Identical moves are not a conflict: two people adding the same
            // field with the same type have agreed, not disagreed.
            if same(upstream, local) {
                None
            } else {
                Some((MergeVerdict::Conflict, describe_conflict(upstream, local)))
            }
        }
    }
}

/// Whether two optional field definitions are the same. Compared by their serialised form:
/// `FieldDef` carries unknown `x-` attributes in a flattened map, and a comparison that only
/// looked at the named fields would call two definitions equal while an extension differed.
fn same(a: Option<&FieldDef>, b: Option<&FieldDef>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => match (serde_json::to_value(a), serde_json::to_value(b)) {
            (Ok(a), Ok(b)) => a == b,
            // Not `.ok() == .ok()`, which maps two *failures* to `None == None` and calls them
            // equal, and "equal" here means the field never enters the plan, so a genuine
            // upstream change would be neither reported nor applied.
            //
            // No input reaches this arm today: every `FieldDef` member serialises, and even a
            // non-finite `minimum`/`maximum` yields `Ok(Null)` from `serde_json` rather than an
            // error (measured, not assumed). It is written this way because an error is evidence
            // of nothing, and a silently dropped field is the worst possible way to find that
            // out later. Deliberately untested: there is no way to construct the input.
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

/// Produces the definition a plan describes: upstream's version of everything it changed
/// alone, the workspace's version of everything it changed alone.
///
/// Refuses a plan with conflicts. There is no answer to apply for those (that is what a
/// conflict means), and applying the rest would leave a definition that is neither what the
/// merge produced nor what was there before, with no record of which fields were skipped.
///
/// The result is a definition, not a stored schema. Whether writing it mints a new version is
/// a separate decision, and one this function deliberately does not make.
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
                .map(|f| yorishiro_core::error::ValidationDetail {
                    field: format!("/{}/{}", f.entity_type, f.field),
                    problem: f.detail.clone(),
                })
                .collect(),
            hint: format!(
                "resolve {} before applying; a partially applied merge is a definition \
                 neither side asked for",
                fields.join(", ")
            ),
        });
    }

    // Start from the workspace's own definition: everything it holds stays unless the plan
    // says upstream changed that field alone. Starting from upstream instead would silently
    // drop every local addition, which is the failure the base exists to prevent.
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
                        // The entity type may not exist locally yet: an upstream addition of
                        // a whole type arrives field by field.
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
                        // Removed upstream, untouched locally: the plan calls that an update,
                        // and the update is the removal.
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
#[path = "../../tests/services/merge.rs"]
mod tests;
