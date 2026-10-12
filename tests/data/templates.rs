use yorishiro::data::templates::{TEMPLATES, parse};
use yorishiro::models::schema_schemas::metaschema::validate_definition;

#[test]
fn every_builtin_asset_is_a_schema_definition() {
    for template in TEMPLATES {
        let definition = parse(template);
        assert!(!definition.entity_types.is_empty(), "{}", template.id);
    }
}

#[test]
fn every_builtin_definition_passes_validation() {
    assert_eq!(TEMPLATES.len(), 8);
    for template in TEMPLATES {
        validate_definition(&parse(template)).unwrap_or_else(|error| {
            panic!("{} is invalid: {error}", template.id);
        });
    }
}

#[test]
fn task_management_project_and_task_fields_are_unchanged() {
    let template = TEMPLATES
        .iter()
        .find(|template| template.id == "task-management")
        .unwrap();
    let definition = parse(template);
    let json = serde_json::to_value(definition).unwrap();
    assert_eq!(
        json["entity_types"]["project"],
        serde_json::json!({
            "description": "A project grouping related tasks",
            "fields": {
                "title": {"type": "string", "required": true, "description": "Project name", "x-embed": true, "maxLength": 200},
                "status": {"type": "string", "enum": ["active", "on_hold", "done"], "required": true, "description": "Current project status"},
                "due": {"type": "string", "format": "date", "required": false, "description": "Target completion date"},
                "description": {"type": "string", "required": false, "description": "What this project is about", "x-embed": true, "x-ui": {"widget": "textarea"}}
            }
        })
    );
    assert_eq!(
        json["entity_types"]["task"],
        serde_json::json!({
            "description": "A single work item",
            "fields": {
                "title": {"type": "string", "required": true, "description": "What needs to be done", "x-embed": true, "maxLength": 200},
                "note": {"type": "string", "required": false, "description": "Additional details or context", "x-embed": true, "x-ui": {"widget": "textarea"}},
                "done": {"type": "boolean", "required": false, "description": "Whether the task is completed", "default": false},
                "priority": {"type": "string", "enum": ["low", "medium", "high", "urgent"], "required": false, "description": "Task priority level", "default": "medium"},
                "due": {"type": "string", "format": "date", "required": false, "description": "Deadline for the task"},
                "tags": {"type": "array", "items": {"type": "string"}, "required": false, "description": "Labels for categorization", "uniqueItems": true}
            }
        })
    );
}
