use serde_json::json;

use crate::metaschema::{MergeVerdict, MetaSchemaDefinition, three_way};

/// Builds a definition with one entity type whose fields are given as JSON.
fn def(fields: serde_json::Value) -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": { "task": { "fields": fields } }
    }))
    .unwrap()
}

fn verdict_for(plan: &crate::metaschema::MergePlan, field: &str) -> Option<MergeVerdict> {
    plan.fields
        .iter()
        .find(|f| f.field == field)
        .map(|f| f.verdict)
}

/// Upstream added a field the workspace has never had. Taking it adds structure without
/// touching anything stored.
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

/// The workspace's own field. Following the template must not delete it — this is the case
/// the two-way comparison could not express, since "absent upstream" looked the same as
/// "removed upstream".
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

/// Upstream changed a field nobody here touched. No local work to lose.
#[test]
fn an_upstream_change_to_an_untouched_field_is_taken() {
    let base = def(json!({ "title": { "type": "string" } }));
    let upstream = def(json!({ "title": { "type": "string", "maxLength": 200 } }));
    let local = base.clone();

    let plan = three_way(&base, &upstream, &local);

    assert_eq!(verdict_for(&plan, "title"), Some(MergeVerdict::AutoUpdate));
}

/// Both sides changed the same field differently. Nothing here picks a side: whichever lost
/// would leave the entities written against it failing validation.
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

/// Both added the same field the same way. That is agreement, not a conflict — reporting it
/// would make an operator adjudicate a decision already shared.
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

/// An entity type present only upstream is as much a difference as a field, and each of its
/// fields is reported.
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

/// Fields differing only in an unknown `x-` extension still differ. FieldDef keeps those in a
/// flattened map, so a comparison over the named fields alone would call these equal.
#[test]
fn an_extension_attribute_is_part_of_the_comparison() {
    let base = def(json!({ "title": { "type": "string" } }));
    let upstream = def(json!({ "title": { "type": "string", "x-ui": { "widget": "textarea" } } }));
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
