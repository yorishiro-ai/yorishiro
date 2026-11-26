use crate::data::time::Timezone;
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateWorkspaceRequest {
    pub(crate) name: String,
    /// Explicit operational cap on entities, independent of any additional deployment policy.
    /// Omit for no explicit cap.
    pub(crate) max_entities: Option<i32>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct WorkspaceDetail {
    pub(crate) id: Uuid,
    pub(crate) tenant_id: Uuid,
    pub(crate) name: String,
    pub(crate) max_entities: Option<i32>,
    pub(crate) effective_max_entities: Option<i32>,
    pub(crate) schema_id: Option<Uuid>,
    pub(crate) created_at: DateTime<Timezone>,
    pub(crate) entity_count: i64,
    pub(crate) relation_count: i64,
    /// Currently *active* schemas only (one per distinct schema name), not a raw row count, which would also include archived versions.
    pub(crate) schema_count: i64,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateWorkspaceRequest {
    pub(crate) name: Option<String>,
    /// When true, replace the manual cap, including clearing it with null.
    #[serde(default)]
    pub(crate) replace_max_entities: bool,
    pub(crate) max_entities: Option<i32>,
}
