use serde::Serialize;

use crate::error::YorishiroError;
use crate::metaschema::MetaSchemaDefinition;

struct BuiltinTemplate {
    id: &'static str,
    source: &'static str,
}

const TEMPLATES: &[BuiltinTemplate] = &[
    BuiltinTemplate {
        id: "general-notes",
        source: include_str!("../templates/general-notes.json"),
    },
    BuiltinTemplate {
        id: "task-management",
        source: include_str!("../templates/task-management.json"),
    },
    BuiltinTemplate {
        id: "worldbuilding",
        source: include_str!("../templates/worldbuilding.json"),
    },
    BuiltinTemplate {
        id: "software-adr",
        source: include_str!("../templates/software-adr.json"),
    },
];

/// Summary of a built-in schema template, returned by `list_templates` so a caller can pick a `template_id` without first fetching every template's full definition.
#[derive(Debug, Clone, Serialize)]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

fn parse(template: &BuiltinTemplate) -> MetaSchemaDefinition {
    serde_json::from_str(template.source)
        .unwrap_or_else(|err| panic!("built-in template '{}' failed to parse: {err}", template.id))
}

pub fn list_templates() -> Vec<TemplateSummary> {
    TEMPLATES
        .iter()
        .map(|template| {
            let definition = parse(template);
            TemplateSummary {
                id: template.id.to_string(),
                name: definition.name,
                description: definition.description,
            }
        })
        .collect()
}

pub fn get_template(id: &str) -> Result<MetaSchemaDefinition, YorishiroError> {
    TEMPLATES
        .iter()
        .find(|template| template.id == id)
        .map(parse)
        .ok_or_else(|| YorishiroError::not_found(format!("no template named '{id}'")))
}
