use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

/// A row in the `entities` table. `embedding` is managed separately by the
/// search/embedding pipeline, so this module's CRUD doesn't touch it. `created_by`/
/// `updated_by` are `None` for entities touched by an unattributed (service/automation) API
/// key, since there's no user to record. `Deserialize` is derived so this can be read back
/// from a JSONL export (see `repositories::import`); import treats `id`, `schema_version`,
/// `created_at`/`updated_at` as informational only (a fresh row is always inserted).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct EntityRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub schema_id: Uuid,
    pub schema_version: i32,
    pub entity_type: String,
    #[schema(value_type = Object)]
    pub data: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub updated_by: Option<Uuid>,
}

pub struct CreateEntityInput {
    pub schema_name: String,
    pub entity_type: String,
    pub data: Value,
}

pub const DEFAULT_LIST_LIMIT: i64 = 50;

pub struct ListEntitiesQuery {
    pub entity_type: Option<String>,
    /// JSONB containment filter (`data @> filter`), e.g. `{"status": "active"}`.
    pub filter: Option<Value>,
    /// Restricts results to entities created against this schema version. Entities keep the
    /// version they were written against, so this selects the entities a given version
    /// produced rather than the ones that would validate against it today.
    pub schema_version: Option<i32>,
    pub limit: i64,
    pub offset: i64,
}

impl Default for ListEntitiesQuery {
    fn default() -> Self {
        Self {
            entity_type: None,
            filter: None,
            schema_version: None,
            limit: DEFAULT_LIST_LIMIT,
            offset: 0,
        }
    }
}

#[cfg(test)]
#[path = "../../tests/models/entities.rs"]
mod tests;

/// How one entity stands relative to the active version of its schema.
///
/// Entities are migrated lazily: a schema gaining a version does not rewrite the rows written
/// against earlier ones, and an update validates against the version the entity was created
/// with. That is deliberate — it is what stops a schema change from invalidating stored data —
/// but it leaves a reader unable to tell whether a field is absent because nobody filled it in
/// or because it did not exist when the entity was written.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct EntityDrift {
    pub entity_id: Uuid,
    pub entity_type: String,
    /// The version this entity was written against.
    pub schema_version: i32,
    /// The newest active version of the same schema.
    pub active_schema_version: i32,
    /// Fields the active version defines that this entity's version did not. Empty when the
    /// entity is current, and empty as well when the newer version only changed fields the
    /// entity already carries.
    pub missing_fields: Vec<DriftField>,
}

/// A field an entity predates.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DriftField {
    pub name: String,
    /// The field's type in the active version, so a caller can tell what would go there.
    /// Serializes to the same spelling the schema uses.
    pub r#type: crate::metaschema::FieldTypeName,
    /// Whether the active version marks it required. A required field an old entity lacks is
    /// the case worth surfacing: the entity is valid under its own version and would not be
    /// under the current one.
    pub required: bool,
}
