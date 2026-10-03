use serde::Serialize;

use crate::models::schema_schemas::metaschema::VersioningDiff;
use crate::models::schema_schemas::{MergeDiffSummary, SchemaRecord};

#[derive(Debug, Serialize)]
pub(crate) struct MergeResponse {
    pub(crate) schema: SchemaRecord,
    pub(crate) diff: VersioningDiff,
    pub(crate) summary: MergeDiffSummary,
}
