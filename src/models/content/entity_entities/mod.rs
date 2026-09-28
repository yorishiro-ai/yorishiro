use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

mod backend;
mod crud;
mod migration;
mod snapshots;
mod validation;

pub use super::_entities::entity_entities::{ActiveModel, Entity, Model};
use crate::metaschema;

pub use crud::{count, create, delete, export_all, get, get_batch, list, update};
pub use migration::{drift, fill_defaults, migration_dry_run};
pub use snapshots::{delete_snapshot, snapshot, undo_job};
pub use validation::validate_data;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::sqlite_generated_id(db, this.id);
        this.updated_at = crate::db::stamped_updated_at(insert, this.updated_at);
        Ok(this)
    }
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}

/// The RLS-scoped request path's view of a `entity_entities` row.
/// Distinct from the generated `Model` because it excludes `embedding` (stored separately in `entity_embeddings`), so SQLite deserialization does not trip on the PgVector type.
/// `created_by`/`updated_by` are `None` for entities touched by an unattributed API key.
#[derive(Clone, Debug, Serialize, Deserialize, sea_orm::FromQueryResult)]
pub struct EntityRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub schema_id: Uuid,
    pub schema_version: i32,
    pub entity_type: String,
    pub data: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub updated_by: Option<Uuid>,
}

impl From<Model> for EntityRecord {
    fn from(row: Model) -> Self {
        EntityRecord {
            id: row.id,
            workspace_id: row.workspace_id,
            schema_id: row.schema_id,
            schema_version: row.schema_version,
            entity_type: row.entity_type,
            data: row.data,
            created_at: row.created_at.into(),
            updated_at: row.updated_at.into(),
            created_by: row.created_by,
            updated_by: row.updated_by,
        }
    }
}

pub struct CreateEntityInput {
    pub schema_name: String,
    pub entity_type: String,
    pub data: Value,
}

pub struct UpdateEntityInput {
    pub id: Uuid,
    pub data: Value,
    pub updated_by: Option<Uuid>,
}

#[derive(Default)]
pub struct ListEntitiesQuery {
    pub entity_type: Option<String>,
    /// JSONB containment filter (`data @> filter`), e.g. `{"status": "active"}`.
    pub filter: Option<Value>,
    /// Restricts results to entities created against this schema version.
    /// Entities keep the version they were written against, so this selects the entities a given version produced, not the ones that would validate against it today.
    pub schema_version: Option<i32>,
    pub page: super::pagination::ListParams,
}

/// How one entity stands relative to the active version of its schema.
///
/// Entities are migrated lazily, so a schema gaining a version does not rewrite rows written against earlier versions.
#[derive(Clone, Debug, Serialize)]
pub struct EntityDrift {
    pub entity_id: Uuid,
    pub entity_type: String,
    /// The version this entity was written against.
    pub schema_version: i32,
    /// The newest active version of the same schema.
    pub active_schema_version: i32,
    /// Fields the active version defines that this entity's version did not.
    /// Empty when the entity is current, and empty as well when the newer version only changed fields the entity already carries.
    pub missing_fields: Vec<DriftField>,
}

/// A field an entity predates.
#[derive(Clone, Debug, Serialize)]
pub struct DriftField {
    pub name: String,
    /// The field's type in the active version, so a caller can tell what would go there.
    pub r#type: metaschema::FieldTypeName,
    /// Whether the active version marks it required.
    /// A required field an old entity lacks is the case worth surfacing: the entity is valid under its own version and would not be under the current one.
    pub required: bool,
}

/// What a batch migration would find without doing it.
#[derive(Clone, Debug, Serialize)]
pub struct MigrationDryRun {
    pub schema_name: String,
    /// The version everything would be brought to.
    pub active_version: i32,
    pub total_entities: i64,
    /// Already on the active version.
    /// Nothing to do for these.
    pub current: i64,
    /// On an older version, but missing no field the active version requires: they validate as they stand and only their version marker is behind.
    pub behind_but_valid: i64,
    /// On an older version and missing at least one field the active version requires.
    /// These are what a batch migration has to fill in.
    pub needs_values: i64,
    /// Per entity type, so an operator can see whether the work is spread or concentrated.
    pub by_entity_type: Vec<DryRunByType>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DryRunByType {
    pub entity_type: String,
    pub behind: i64,
    pub needs_values: i64,
    /// The required fields those entities lack, so the report names the work rather than only counting it.
    pub missing_required: Vec<String>,
}

/// Result of a fill-defaults operation.
#[derive(Clone, Serialize)]
pub struct FillDefaultsReport {
    pub schema_name: String,
    pub job_id: Uuid,
    pub entities_updated: i64,
    pub fields_filled: i64,
}

/// An entity's data as it stood before something overwrote it.
#[derive(Clone, Serialize, sea_orm::FromQueryResult)]
pub struct EntitySnapshot {
    pub id: Uuid,
    /// Groups the snapshots taken by one operation, so a batch is undone as a batch.
    pub job_id: Uuid,
    pub entity_id: Uuid,
    pub schema_id: Uuid,
    pub schema_version: i32,
    pub data: Value,
    pub created_at: DateTime<Utc>,
}

/// What undoing a job put back.
#[derive(Clone, Serialize)]
pub struct UndoReport {
    pub job_id: Uuid,
    /// Entities restored to the data they held before.
    pub restored: i64,
    /// Snapshots whose entity no longer exists.
    /// Counted rather than treated as an error: a batch partially undone leaves a workspace in a state nobody chose, and an entity deleted since is not a reason to refuse the rest.
    pub missing: i64,
}
