use crate::models::schema_schemas::SchemaRecord;
use crate::models::schema_schemas::metaschema::{MetaSchemaDefinition, VersioningDiff};
use serde::{Deserialize, Serialize};

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct CreateSchemaResponse {
    pub(crate) schema: SchemaRecord,
    pub(crate) diff: VersioningDiff,
}

/// Either an inline schema definition, or a reference to a template.
///
/// `template_id` is either a built-in id (`"task-management"`, see `GET /api/templates`) or a UUID from the tenant's template library (`GET /api/template-library`).
///
/// A flat `MetaSchemaDefinition` body is the inline form, so the enum is untagged.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub(crate) enum CreateSchemaRequest {
    Definition(MetaSchemaDefinition),
    Template { template_id: String },
}
