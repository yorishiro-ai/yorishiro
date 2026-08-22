use chrono::{DateTime, Utc};
use sea_orm::QueryOrder;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use super::_entities::content_schemas::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::MetaSchemaDefinition;

pub type ContentSchemas = Entity;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        Ok(self)
    }
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}

/// Following a template that still exists.
pub const ORIGIN_STATUS_LINKED: &str = "linked";

/// Not following anything: written by hand, or following a template that has since been deleted.
pub const ORIGIN_STATUS_DETACHED: &str = "detached";

/// Represents a row in the `content_schemas` table.
/// `definition` is JSONB in the DB, but the application layer always treats it as a parsed `MetaSchemaDefinition`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub version: i32,
    pub definition: MetaSchemaDefinition,
    pub status: String,
    pub origin_template_id: Option<Uuid>,
    pub origin_status: String,
    pub origin_snapshot: Option<MetaSchemaDefinition>,
    pub created_at: DateTime<Utc>,
}

impl TryFrom<Model> for SchemaRecord {
    type Error = YorishiroError;

    fn try_from(row: Model) -> Result<Self, Self::Error> {
        Ok(SchemaRecord {
            id: row.id,
            tenant_id: row.tenant_id,
            workspace_id: row.workspace_id,
            name: row.name,
            version: row.version,
            definition: serde_json::from_value(row.definition).internal()?,
            status: row.status,
            origin_template_id: row.origin_template_id,
            origin_status: row.origin_status,
            origin_snapshot: row
                .origin_snapshot
                .map(serde_json::from_value)
                .transpose()
                .internal()?,
            created_at: row.created_at.into(),
        })
    }
}

/// Fetches the currently active schema (the latest version with status='active') for the given workspace and name.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`, so it
/// takes anything implementing `ConnectionTrait` (a `DatabaseTransaction`, in practice).
pub async fn get_active_schema(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    name: &str,
) -> Result<SchemaRecord, YorishiroError> {
    use super::_entities::content_schemas::Column;

    let row = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Name.eq(name))
        .filter(Column::Status.eq("active"))
        .order_by_desc(Column::Version)
        .one(conn)
        .await
        .internal()?;

    match row {
        Some(row) => row.try_into(),
        None => Err(YorishiroError::not_found(format!(
            "no active schema named '{name}'"
        ))),
    }
}

/// Fetches a specific schema version by id (used to resolve the version an entity references).
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`, so it
/// takes anything implementing `ConnectionTrait` (a `DatabaseTransaction`, in practice).
pub async fn get_by_id(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<SchemaRecord, YorishiroError> {
    use super::_entities::content_schemas::Column;

    let row = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Id.eq(schema_id))
        .one(conn)
        .await
        .internal()?;

    match row {
        Some(row) => row.try_into(),
        None => Err(YorishiroError::not_found(format!(
            "schema '{schema_id}' was not found"
        ))),
    }
}
