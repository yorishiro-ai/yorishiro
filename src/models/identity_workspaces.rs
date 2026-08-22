use sea_orm::entity::prelude::*;
use sqlx::PgConnection;

pub use super::_entities::identity_workspaces::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, YorishiroError};

pub type IdentityWorkspaces = Entity;

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

/// A workspace with no schema yet. Entity writes are refused with a 422 that says so.
pub const WORKSPACE_STATUS_SCHEMA_PENDING: &str = "schema_pending";

/// A workspace that owns at least one schema.
pub const WORKSPACE_STATUS_ACTIVE: &str = "active";

/// Whether the workspace is still waiting for its first schema.
///
/// Runs on the RLS-scoped raw connection a request handler holds via `Authorized::conn()`, not
/// through the SeaORM entity layer: see the "which pool" rule in this repository's Loco-rebuild
/// notes (product `CLAUDE.md`).
pub async fn is_schema_pending(
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<bool, YorishiroError> {
    let status: Option<(String,)> =
        sqlx::query_as("SELECT status FROM identity_workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&mut *conn)
            .await
            .internal()?;

    Ok(status.is_some_and(|(s,)| s == WORKSPACE_STATUS_SCHEMA_PENDING))
}
