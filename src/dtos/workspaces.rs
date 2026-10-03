use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct CreateWorkspaceRequest {
    pub name: String,
    /// Cap on the number of entities this workspace may hold.
    /// Omit for unlimited.
    pub max_entities: Option<i32>,
    /// Schema to associate with this workspace.
    /// Omit to leave it unset.
    #[serde(default)]
    pub schema_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct WorkspaceDetail {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub max_entities: Option<i32>,
    pub schema_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub entity_count: i64,
    pub relation_count: i64,
    /// Currently *active* schemas only (one per distinct schema name), not a raw row count, which would also include archived versions.
    pub schema_count: i64,
}
