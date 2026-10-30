use chrono::{DateTime, Utc};
use sea_orm::ExprTrait;
use sea_orm::QuerySelect;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

mod backend;
mod crud;
mod migration;
mod snapshots;
pub(crate) use snapshots::require_job_snapshots;
mod validation;

use crate::models::_entities::entity_entities::Column;
pub use crate::models::_entities::entity_entities::{ActiveModel, Entity, Model};
use crate::models::schema_schemas::metaschema;

/// Written by hand because the generated `Model` carries `embedding_sync_token`, which the entity hardening script treats as secret-bearing and therefore strips `Debug` from.
/// The token is a concurrency guard, never needed in a log line, so it is the one field left out.
impl std::fmt::Debug for Model {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EntityEntity")
            .field("id", &self.id)
            .field("workspace_id", &self.workspace_id)
            .field("schema_id", &self.schema_id)
            .field("schema_version", &self.schema_version)
            .field("entity_type", &self.entity_type)
            .field("data", &self.data)
            .field("created_by", &self.created_by)
            .field("updated_by", &self.updated_by)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

pub(crate) use crud::create_with_quota;
pub use crud::{count, create, delete, export_all, get, get_batch, list, update};
pub(crate) use migration::fill_defaults;
pub use migration::{drift, migration_dry_run};
pub use snapshots::undo_job;
pub use snapshots::{delete_snapshot, snapshot};
pub use validation::validate_data;

pub(crate) async fn ids_for_workspace(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<Uuid>, DbErr> {
    Entity::find()
        .select_only()
        .column(crate::models::_entities::entity_entities::Column::Id)
        .filter(crate::models::_entities::entity_entities::Column::WorkspaceId.eq(workspace_id))
        .into_tuple()
        .all(db)
        .await
}

/// Reads a single entity with its embedding-sync token.
///
/// Used by the embedding sync worker and the public `sync_embedding_for_record`:
/// they need the entity body plus the token that guards `embed_and_write`.
pub(crate) async fn get_with_token(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
) -> Result<EmbeddingSnapshot, DbErr> {
    snapshot_select()
        .filter(Column::Id.eq(entity_id))
        .filter(Column::WorkspaceId.eq(workspace_id))
        .into_model::<EmbeddingSnapshot>()
        .one(db)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound(Entity.table_name().to_string()))
}

/// Reads the given entities with their embedding-sync tokens, whether or not they already have a vector.
///
/// Used by a full reindex, which re-embeds every entity regardless of existing vectors.
pub(crate) async fn get_batch_with_tokens(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    ids: &[Uuid],
) -> Result<Vec<EmbeddingSnapshot>, DbErr> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    snapshot_select()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.is_in(ids.iter().copied()))
        .into_model::<EmbeddingSnapshot>()
        .all(db)
        .await
}

fn snapshot_select() -> Select<Entity> {
    Entity::find().select_only().columns([
        Column::Id,
        Column::SchemaId,
        Column::EntityType,
        Column::Data,
        Column::EmbeddingSyncToken,
    ])
}

/// An entity as the embedding code reads it: the fields it embeds plus the concurrency token.
///
/// `embed_and_write` stores a vector only while the entity still carries this token.
/// The token is not part of `EntityRecord`, so it never reaches an API response.
#[derive(sea_orm::FromQueryResult)]
pub(crate) struct EmbeddingSnapshot {
    pub(crate) id: Uuid,
    pub(crate) schema_id: Uuid,
    pub(crate) entity_type: String,
    pub(crate) data: Value,
    pub(crate) embedding_sync_token: String,
}

/// Finds every entity in a workspace that has no corresponding embedding row.
///
/// An anti-subquery excludes entities with stored embeddings.
pub(crate) async fn missing_embeddings(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<EmbeddingSnapshot>, DbErr> {
    use sea_orm::sea_query::{Alias, Query};
    let embedded = Query::select()
        .column(Alias::new("entity_id"))
        .from(Alias::new(crate::models::entity_embeddings::TABLE))
        .to_owned();
    Entity::find()
        .select_only()
        .columns([
            Column::Id,
            Column::SchemaId,
            Column::EntityType,
            Column::Data,
            Column::EmbeddingSyncToken,
        ])
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Expr::col(Column::Id).not_in_subquery(embedded))
        .into_model::<EmbeddingSnapshot>()
        .all(db)
        .await
}

#[loco_rs::prelude::async_trait]
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

/// A stored entity.
/// `created_by`/`updated_by` are absent for entities touched by an unattributed API key.
//
// Distinct from the generated `Model` because it excludes `embedding` (stored separately in `entity_embeddings`), so SQLite deserialization does not trip on the PgVector type.
#[derive(Debug, Clone, Serialize, Deserialize, sea_orm::FromQueryResult)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
    pub page: crate::models::pagination::ListParams,
}

/// How one entity stands relative to the active version of its schema.
///
/// Entities are migrated lazily, so a schema gaining a version does not rewrite rows written against earlier versions.
#[derive(Debug, Clone, Serialize)]
pub struct EntityDrift {
    pub entity_id: Uuid,
    pub entity_type: String,
    /// The version this entity was written against.
    pub schema_version: i32,
    /// The newest active version of the same schema.
    pub active_schema_version: i32,
    /// Active-version fields absent from the stored data, independent of the entity's schema version.
    /// Present nulls and invalid types are not classified as absent.
    pub missing_fields: Vec<DriftField>,
}

/// An active-version field absent from stored data.
#[derive(Debug, Clone, Serialize)]
pub struct DriftField {
    /// Top-level field name, or an RFC 6901 JSON Pointer for a nested field.
    pub name: String,
    /// The field's type in the active version, so a caller can tell what would go there.
    pub r#type: metaschema::FieldTypeName,
    /// Whether the active version marks it required.
    /// A required field an old entity lacks is the case worth surfacing: the entity is valid under its own version and would not be under the current one.
    pub required: bool,
}

/// What a batch migration would find without doing it.
#[derive(Debug, Clone, Serialize)]
pub struct MigrationDryRun {
    pub schema_name: String,
    /// The version everything would be brought to.
    pub active_version: i32,
    pub total_entities: i64,
    /// Already on the active version.
    /// Nothing to do for these.
    pub current: i64,
    /// On an older version, but missing no required field in existing objects.
    /// This is a presence check, not full active-schema validation of types or constraints.
    pub behind_but_valid: i64,
    /// On an older version and missing at least one field the active version requires.
    /// These are what a batch migration has to fill in.
    pub needs_values: i64,
    /// Per entity type, so an operator can see whether the work is spread or concentrated.
    pub by_entity_type: Vec<DryRunByType>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DryRunByType {
    pub entity_type: String,
    pub behind: i64,
    pub needs_values: i64,
    /// The required fields those entities lack, so the report names the work rather than only counting it.
    pub missing_required: Vec<String>,
}

/// Result of a fill-defaults operation.
#[derive(Clone, Serialize)]
pub(crate) struct FillDefaultsReport {
    pub schema_name: String,
    pub job_id: Uuid,
    pub entities_updated: i64,
    pub fields_filled: i64,
}

/// An entity's data as it stood before something overwrote it.
#[derive(Clone, Serialize, sea_orm::FromQueryResult)]
pub(crate) struct EntitySnapshot {
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UndoReport {
    pub job_id: Uuid,
    /// Entities restored to the data they held before.
    pub restored: i64,
    /// Snapshots whose entity no longer exists.
    /// Counted rather than treated as an error: a batch partially undone leaves a workspace in a state nobody chose, and an entity deleted since is not a reason to refuse the rest.
    pub missing: i64,
}
