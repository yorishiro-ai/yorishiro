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

/// What a batch migration would find, without doing it.
///
/// Migration is lazy: an entity keeps validating against the version it was written with, so a
/// workspace accumulates entities spread across versions. This counts them before anything is
/// touched, because the useful question before a migration is how much of the corpus it would
/// have to fill in — a number that decides whether defaults suffice or whether the work needs
/// a person.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MigrationDryRun {
    pub schema_name: String,
    /// The version everything would be brought to.
    pub active_version: i32,
    pub total_entities: i64,
    /// Already on the active version. Nothing to do for these.
    pub current: i64,
    /// On an older version, but missing no field the active version requires — they validate
    /// as they stand and only their version marker is behind.
    pub behind_but_valid: i64,
    /// On an older version and missing at least one field the active version requires. These
    /// are what a migration has to fill in, and what mode A's defaults or mode B's inference
    /// would be for.
    pub needs_values: i64,
    /// Per entity type, so an operator can see whether the work is spread or concentrated.
    pub by_entity_type: Vec<DryRunByType>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DryRunByType {
    pub entity_type: String,
    pub behind: i64,
    pub needs_values: i64,
    /// The required fields those entities lack, so the report names the work rather than only
    /// counting it.
    pub missing_required: Vec<String>,
}

/// An entity's data as it stood before something overwrote it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct EntitySnapshot {
    pub id: Uuid,
    /// Groups the snapshots taken by one operation, so a batch is undone as a batch.
    pub job_id: Uuid,
    pub entity_id: Uuid,
    pub schema_id: Uuid,
    pub schema_version: i32,
    #[schema(value_type = Object)]
    pub data: Value,
    pub created_at: DateTime<Utc>,
}

/// What undoing a job put back.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UndoReport {
    pub job_id: Uuid,
    /// Entities restored to the data they held before.
    pub restored: i64,
    /// Snapshots whose entity no longer exists. Counted rather than treated as an error: a
    /// batch partially undone leaves a workspace in a state nobody chose, and an entity
    /// deleted since is not a reason to refuse the rest.
    pub missing: i64,
}

/// What filling defaults did.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FillDefaultsReport {
    /// Groups the snapshots taken, so this run can be undone as one.
    pub job_id: Uuid,
    pub schema_name: String,
    /// Entities that gained at least one value.
    pub filled: i64,
    /// Entities that needed a value the active version defines no default for. Left untouched
    /// and counted, because inventing one would be worse than leaving the field absent — a
    /// value nobody chose is indistinguishable from one someone did.
    pub skipped_no_default: i64,
    /// The fields those entities still lack.
    pub still_missing: Vec<String>,
}
