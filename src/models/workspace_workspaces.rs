use sea_orm::Statement;
use sea_orm::entity::prelude::*;
use serde::Serialize;
use std::fmt;
use std::str::FromStr;

pub use super::_entities::workspace_workspaces::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, YorishiroError};

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    /// `id` has a `uuidv7()` column default on PostgreSQL and no default on SQLite; see `crate::db::sqlite_generated_id`.
    async fn before_save<C>(mut self, db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        self.id = crate::db::sqlite_generated_id(db, self.id);
        Ok(self)
    }
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}

/// API-facing workspace record with a typed status.
#[derive(Clone, Debug, Serialize)]
pub struct WorkspaceRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub max_entities: Option<i32>,
    pub status: WorkspaceStatus,
    pub embedding_model: Option<String>,
    pub embedding_dimensions: Option<i32>,
    pub schema_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
}

impl TryFrom<Model> for WorkspaceRecord {
    type Error = YorishiroError;

    fn try_from(model: Model) -> Result<Self, Self::Error> {
        let status = WorkspaceStatus::from_db_str(&model.status).ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!(
                "unknown workspace status: {}",
                model.status
            ))
        })?;
        Ok(Self {
            id: model.id,
            tenant_id: model.tenant_id,
            name: model.name,
            max_entities: model.max_entities,
            status,
            embedding_model: model.embedding_model,
            embedding_dimensions: model.embedding_dimensions,
            schema_id: model.schema_id,
            created_at: model.created_at,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    SchemaPending,
    Active,
}

impl WorkspaceStatus {
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::SchemaPending => "schema_pending",
            Self::Active => "active",
        }
    }

    pub fn from_db_str(value: &str) -> Option<Self> {
        value.parse().ok()
    }
}

impl fmt::Display for WorkspaceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_db_str())
    }
}

impl FromStr for WorkspaceStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "schema_pending" => Ok(Self::SchemaPending),
            "active" => Ok(Self::Active),
            _ => Err(format!("unknown workspace status: {value}")),
        }
    }
}

/// Whether the workspace is still waiting for its first schema.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`, so it takes anything implementing `ConnectionTrait` (a `DatabaseTransaction`, in practice).
pub async fn is_schema_pending(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<bool, YorishiroError> {
    let status = Entity::find_by_id(workspace_id)
        .one(conn)
        .await
        .internal()?
        .map(|model| model.status);

    Ok(status.is_some_and(|s| s == WorkspaceStatus::SchemaPending.as_db_str()))
}

/// Marks a workspace active and records its first schema, idempotently.
///
/// One statement (`COALESCE(schema_id, $new)`), not a read-then-write: two concurrent schema creations must not both see `schema_id` as `NULL` and overwrite each other's write.
///
/// Raw SQL, not `ActiveModel`: `COALESCE(...)` can't be expressed via `Set(...)`, and `yorishiro_app` holds UPDATE only on `workspace_workspaces (status, schema_id)` (a column-level GRANT), so the statement must touch exactly those two columns.
pub async fn mark_active(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<(), YorishiroError> {
    conn.execute_raw(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "UPDATE workspace_workspaces \
         SET status = $1, schema_id = COALESCE(schema_id, $2) \
         WHERE id = $3",
        [
            WorkspaceStatus::Active.as_db_str().into(),
            schema_id.into(),
            workspace_id.into(),
        ],
    ))
    .await
    .internal()?;
    Ok(())
}
