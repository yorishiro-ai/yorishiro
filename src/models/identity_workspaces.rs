use sea_orm::Statement;
use sea_orm::entity::prelude::*;

pub use super::_entities::identity_workspaces::{ActiveModel, Entity, Model};
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

/// A workspace with no schema yet.
/// Entity writes are refused with a 422 that says so.
pub const WORKSPACE_STATUS_SCHEMA_PENDING: &str = "schema_pending";

/// A workspace that owns at least one schema.
pub const WORKSPACE_STATUS_ACTIVE: &str = "active";

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

    Ok(status.is_some_and(|s| s == WORKSPACE_STATUS_SCHEMA_PENDING))
}

/// Marks a workspace active and records its first schema, idempotently.
///
/// One statement (`COALESCE(schema_id, $new)`), not a read-then-write: two concurrent schema creations must not both see `schema_id` as `NULL` and overwrite each other's write.
///
/// Raw SQL, not `ActiveModel`: `COALESCE(...)` can't be expressed via `Set(...)`, and `yorishiro_app` holds UPDATE only on `identity_workspaces (status, schema_id)` (a column-level GRANT), so the statement must touch exactly those two columns.
pub async fn mark_active(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<(), YorishiroError> {
    conn.execute_raw(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "UPDATE identity_workspaces \
         SET status = $1, schema_id = COALESCE(schema_id, $2) \
         WHERE id = $3",
        [
            WORKSPACE_STATUS_ACTIVE.into(),
            schema_id.into(),
            workspace_id.into(),
        ],
    ))
    .await
    .internal()?;
    Ok(())
}
