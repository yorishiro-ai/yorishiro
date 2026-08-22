use sea_orm::entity::prelude::*;

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
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`, so it
/// takes anything implementing `ConnectionTrait` (a `DatabaseTransaction`, in practice).
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
