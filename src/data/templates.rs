use serde::Serialize;

use crate::error::YorishiroError;
use crate::models::schema_schemas::metaschema::MetaSchemaDefinition;

pub struct BuiltinTemplate {
    pub id: &'static str,
    source: &'static str,
}

pub const TEMPLATES: &[BuiltinTemplate] = &[
    BuiltinTemplate {
        id: "general-notes",
        source: include_str!("../../data/templates/general-notes.json"),
    },
    BuiltinTemplate {
        id: "task-management",
        source: include_str!("../../data/templates/task-management.json"),
    },
    BuiltinTemplate {
        id: "worldbuilding",
        source: include_str!("../../data/templates/worldbuilding.json"),
    },
    BuiltinTemplate {
        id: "software-adr",
        source: include_str!("../../data/templates/software-adr.json"),
    },
];

/// Summary of a built-in schema template, returned by `list_templates` so a caller can pick a `template_id` without first fetching every template's full definition.
#[derive(Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

///
/// # Panics
/// Panics if an internal invariant required by this operation is violated.
pub fn parse(template: &BuiltinTemplate) -> MetaSchemaDefinition {
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

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub fn get_template(id: &str) -> Result<MetaSchemaDefinition, YorishiroError> {
    TEMPLATES
        .iter()
        .find(|template| template.id == id)
        .map(parse)
        .ok_or_else(|| YorishiroError::not_found(format!("no template named '{id}'")))
}
