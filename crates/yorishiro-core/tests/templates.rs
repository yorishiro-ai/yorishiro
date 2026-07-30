use yorishiro_core::YorishiroError;
use yorishiro_core::templates::{get_template, list_templates};

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
