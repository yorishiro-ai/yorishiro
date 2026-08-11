use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::models::entities::EntityRecord;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct RelationRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    #[schema(value_type = Object)]
    pub properties: Value,
    /// `active`, `deprecated` or `archived`. Traversal follows `active` relations only.
    pub status: String,
    pub created_at: DateTime<Utc>,
}

/// The state a relation is created in, and the only one traversal follows.
pub const RELATION_STATUS_ACTIVE: &str = "active";

/// Every state a relation may hold, matching the check constraint on `content.relations`.
pub const RELATION_STATUSES: [&str; 3] = ["active", "deprecated", "archived"];

/// Whether `status` names a state a relation may hold. Callers validate before writing so an
/// unknown value is a 422 naming the field, not a constraint violation surfacing as a 500.
pub fn is_valid_relation_status(status: &str) -> bool {
    RELATION_STATUSES.contains(&status)
}

pub struct CreateRelationInput {
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    pub properties: Value,
}

pub const DEFAULT_LIST_LIMIT: i64 = 50;

pub struct ListRelationsQuery {
    pub source_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub relation_type: Option<String>,
    /// Restricts the listing to one state. `None` lists every state, so a caller that has not
    /// heard of `status` still sees deprecated and archived relations rather than silently
    /// losing rows it used to get.
    pub status: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl Default for ListRelationsQuery {
    fn default() -> Self {
        Self {
            source_id: None,
            target_id: None,
            relation_type: None,
            status: None,
            limit: DEFAULT_LIST_LIMIT,
            offset: 0,
        }
    }
}

pub const DEFAULT_NEIGHBORS_LIMIT: i64 = 20;

/// A relation together with the entity on the other end of it, relative to the entity
/// `neighbors` was called for. `direction` is `"out"` when the queried entity is the
/// relation's source (the neighbor is the target) and `"in"` when it's the target (the
/// neighbor is the source).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Neighbor {
    pub relation_id: Uuid,
    pub relation_type: String,
    pub direction: String,
    #[schema(value_type = Object)]
    pub properties: Value,
    pub entity: EntityRecord,
}

#[cfg(test)]
#[path = "../../tests/models/relations.rs"]
mod tests;
