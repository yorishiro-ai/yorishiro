use serde::Serialize;

use crate::models::schema_schemas::metaschema::VersioningDiff;
use crate::models::schema_schemas::{MergeDiffSummary, SchemaRecord};

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct MergeResponse {
    pub(crate) schema: SchemaRecord,
    pub(crate) diff: VersioningDiff,
    pub(crate) summary: MergeDiffSummary,
}
