use crate::time::Timezone;
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GrantWorkspaceMemberRequest {
    pub role: crate::models::workspace_memberships::WorkspaceRole,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateWorkspaceRequest {
    pub name: String,
    /// Explicit operational cap on entities, independent of any additional deployment policy.
    /// Omit for no explicit cap.
    pub max_entities: Option<i32>,
    /// Schema to associate with this workspace.
    /// Omit to leave it unset.
    #[serde(default)]
    pub schema_id: Option<Uuid>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WorkspaceDetail {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub max_entities: Option<i32>,
    pub effective_max_entities: Option<i32>,
    pub schema_id: Option<Uuid>,
    pub created_at: DateTime<Timezone>,
    pub entity_count: i64,
    pub relation_count: i64,
    /// Currently *active* schemas only (one per distinct schema name), not a raw row count, which would also include archived versions.
    pub schema_count: i64,
}
