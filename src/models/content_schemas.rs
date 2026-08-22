use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgConnection;
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

#[derive(sqlx::FromRow)]
struct SchemaRow {
    id: Uuid,
    tenant_id: Uuid,
    workspace_id: Uuid,
    name: String,
    version: i32,
    definition: Value,
    status: String,
    origin_template_id: Option<Uuid>,
    origin_status: String,
    origin_snapshot: Option<Value>,
    created_at: DateTime<Utc>,
}

impl SchemaRow {
    fn into_record(self) -> Result<SchemaRecord, YorishiroError> {
        let definition = serde_json::from_value(self.definition).internal()?;
        Ok(SchemaRecord {
            id: self.id,
            tenant_id: self.tenant_id,
            workspace_id: self.workspace_id,
            name: self.name,
            version: self.version,
            definition,
            status: self.status,
            origin_template_id: self.origin_template_id,
            origin_status: self.origin_status,
            origin_snapshot: self
                .origin_snapshot
                .map(serde_json::from_value)
                .transpose()
                .internal()?,
            created_at: self.created_at,
        })
    }
}

/// Fetches the currently active schema (the latest version with status='active') for the given workspace and name.
///
/// Runs on the RLS-scoped raw connection a request handler holds via `Authorized::conn()`, not
/// through the SeaORM entity layer: see the "which pool" rule in this repository's Loco-rebuild
/// notes (product `CLAUDE.md`).
pub async fn get_active_schema(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    name: &str,
) -> Result<SchemaRecord, YorishiroError> {
    let row: Option<SchemaRow> = sqlx::query_as(
        "SELECT id, tenant_id, workspace_id, name, version, definition, status, \
         origin_template_id, origin_status, origin_snapshot, created_at \
         FROM content_schemas \
         WHERE workspace_id = $1 AND name = $2 AND status = 'active' \
         ORDER BY version DESC LIMIT 1",
    )
    .bind(workspace_id)
    .bind(name)
    .fetch_optional(&mut *conn)
    .await
    .internal()?;

    match row {
        Some(row) => row.into_record(),
        None => Err(YorishiroError::not_found(format!(
            "no active schema named '{name}'"
        ))),
    }
}
