use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::metaschema::MetaSchemaDefinition;

/// A workspace's own copy of its tenant's schema.
///
/// A workspace without one of these uses its tenant's schema directly. Forking gives it a copy
/// it can edit without the edit reaching the tenant's other workspaces.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceSchemaRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    /// The `content.schemas` row this was copied from.
    pub source_id: Uuid,
    /// The source's version at fork time. Compared against the source's current active version
    /// to tell whether the tenant's schema has moved on since.
    pub source_version: i32,
    pub definition: MetaSchemaDefinition,
    /// Set once the fork's definition has been edited away from the copy it started as.
    pub customized: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
