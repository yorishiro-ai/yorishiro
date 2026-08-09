use crate::YorishiroError;
use crate::metaschema::validate_definition;
use crate::templates::{get_template, list_templates};

#[test]
fn lists_the_built_in_task_management_template() {
    let templates = list_templates();
    assert!(templates.iter().any(|t| t.id == "task-management"));
}

#[test]
fn fetches_a_template_by_id() {
    let definition = get_template("task-management").unwrap();
    assert_eq!(definition.name, "task-management");
    assert!(definition.entity_types.contains_key("task"));
    assert!(definition.entity_types.contains_key("project"));
}

#[test]
fn reports_not_found_for_an_unknown_template_id() {
    let err = get_template("does-not-exist").unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

/// Every built-in template must pass the same validator a user-submitted schema goes through.
/// `templates.rs` itself only parses the embedded JSON, so without this a template that parses
/// but violates a metaschema rule (a relation pointing at an undeclared entity type, a `format`
/// on a non-string field) would ship and only fail when someone tried to register it.
#[test]
fn every_built_in_template_passes_metaschema_validation() {
    let templates = list_templates();
    assert!(!templates.is_empty(), "no built-in templates were listed");

    for summary in templates {
        let definition = get_template(&summary.id).unwrap_or_else(|err| {
            panic!("built-in template '{}' failed to load: {err}", summary.id)
        });
        assert!(
            validate_definition(&definition).is_ok(),
            "built-in template '{}' failed metaschema validation: {:?}",
            summary.id,
            validate_definition(&definition)
        );
    }
}
