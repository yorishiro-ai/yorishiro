use crate::metaschema::{MetaSchemaDefinition, VersioningDiff};
use crate::models::schema_schemas::SchemaRecord;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct CreateSchemaResponse {
    pub schema: SchemaRecord,
    pub diff: VersioningDiff,
}

/// Either an inline schema definition, or a reference to a template.
///
/// `template_id` accepts both kinds of template, because a caller holding an id should not have to know which kind it is: a built-in id (`"task-management"`, see `GET /api/templates`) served from the binary, or a UUID from the tenant's template library (`GET /api/template-library`).
///
/// Untagged so existing clients posting a flat `MetaSchemaDefinition` body keep working unchanged.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum CreateSchemaRequest {
    Definition(MetaSchemaDefinition),
    Template { template_id: String },
}
