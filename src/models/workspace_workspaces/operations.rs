//! Workspace reads and lifecycle operations.

use crate::error::OptionExt;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait, PaginatorTrait, QueryFilter,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_workspaces::Column;

use super::{Entity, Model};

/// Looks up the tenant that owns a workspace, for workspace authorization.
pub(crate) async fn get_workspace_tenant(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Uuid, YorishiroError> {
    Entity::find_by_id(workspace_id)
        .one(conn)
        .await
        .internal()?
        .map(|w| w.tenant_id)
        .or_not_found("workspace not found")
}

/// The advisory-lock key serializing every operation that counts a tenant's workspaces before writing.
/// `create_workspace` and `delete_workspace` share it because each decides from a count the other changes.
pub(crate) fn workspace_count_lock_key(tenant_id: Uuid) -> String {
    format!("workspace-count:{tenant_id}")
}

/// Fetches a workspace by id.
pub(crate) async fn get_workspace(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Model, YorishiroError> {
    Entity::find_by_id(workspace_id)
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| {
            YorishiroError::not_found(format!("workspace '{workspace_id}' was not found"))
        })
}

/// Deletes a workspace, refusing to remove a tenant's last one.
///
/// The advisory lock serializes deletes in one tenant before the count, so two requests racing to delete its last two workspaces cannot both see a spare one.
/// `conn` is a transaction because that lock is transaction-scoped.
pub(crate) async fn delete_workspace(
    conn: &DatabaseTransaction,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    let workspace = get_workspace(conn, workspace_id).await?;

    crate::db::lock_for_update(conn, &workspace_count_lock_key(workspace.tenant_id))
        .await
        .internal()?;

    let remaining = Entity::find()
        .filter(Column::TenantId.eq(workspace.tenant_id))
        .count(conn)
        .await
        .internal()?;
    if remaining <= 1 {
        return Err(YorishiroError::Conflict {
            message: "cannot delete a tenant's only remaining workspace".into(),
        });
    }

    Entity::delete_by_id(workspace_id)
        .exec(conn)
        .await
        .map_err(|err| {
            // The foreign key fences concurrent fork creation, so no preflight count is needed.
            YorishiroError::conflict_on_fk(
                err,
                "workspace is still referenced by schema forks; remove those forks before deleting it",
            )
        })?;
    Ok(())
}
