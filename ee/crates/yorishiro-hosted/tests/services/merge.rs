use serde_json::json;

use crate::services::merge::{MergeVerdict, apply_plan, three_way};
use yorishiro_core::YorishiroError;
use yorishiro_core::metaschema::MetaSchemaDefinition;

/// Builds a definition with one entity type whose fields are given as JSON.
fn def(fields: serde_json::Value) -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": { "task": { "fields": fields } }
    }))
    .unwrap()
}

fn verdict_for(plan: &crate::services::merge::MergePlan, field: &str) -> Option<MergeVerdict> {
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
/// Following the template must not delete it: this is the case the two-way comparison could not express, since "absent upstream" looked the same as "removed upstream".
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
/// FieldDef keeps those in a flattened map, so a comparison over the named fields alone would call these equal.
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

/// The whole point of three-way: upstream's addition arrives and the workspace's own survives.
/// A two-way merge could not do both.
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
