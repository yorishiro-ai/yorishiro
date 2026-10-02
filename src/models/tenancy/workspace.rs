//! Workspace reads and lifecycle operations.

use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use uuid::Uuid;

use super::WorkspaceSummary;
use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_workspaces;

/// Every workspace under `tenant_id`, for the signup response (which workspaces the new member can now log into).
pub(crate) async fn list_workspaces(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<WorkspaceSummary>, YorishiroError> {
    use crate::models::_entities::workspace_workspaces;

    let workspaces = workspace_workspaces::Entity::find()
        .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
        .order_by_asc(workspace_workspaces::Column::CreatedAt)
        .limit(page.limit() as u64)
        .offset(page.offset() as u64)
        .all(conn)
        .await
        .internal()?;

    Ok(workspaces
        .into_iter()
        .map(|w| WorkspaceSummary {
            id: w.id,
            name: w.name,
        })
        .collect())
}
/// A workspace's id and owning tenant, for `/auth/login`'s explicit `workspace_id` path.
pub(crate) async fn get_workspace_tenant(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Uuid, YorishiroError> {
    use crate::models::_entities::workspace_workspaces;

    workspace_workspaces::Entity::find_by_id(workspace_id)
        .one(conn)
        .await
        .internal()?
        .map(|w| w.tenant_id)
        .ok_or_else(|| YorishiroError::not_found("workspace not found"))
}

/// The advisory-lock key serializing every operation that counts a tenant's workspaces before writing.
/// `create_workspace` and `delete_workspace` share it deliberately: both decide from a count that the other invalidates, so they have to serialize against each other and not merely against themselves.
pub(super) fn workspace_count_lock_key(tenant_id: Uuid) -> String {
    format!("workspace-count:{tenant_id}")
}
/// Fetches a workspace by id.
pub(crate) async fn get_workspace(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<workspace_workspaces::Model, YorishiroError> {
    use crate::models::_entities::workspace_workspaces;

    workspace_workspaces::Entity::find_by_id(workspace_id)
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| {
            YorishiroError::not_found(format!("workspace '{workspace_id}' was not found"))
        })
}

/// Deletes a workspace, refusing to remove a tenant's last one.
///
/// `db::lock_for_update` serializes concurrent deletes against the same tenant before counting its workspaces, so two requests racing to delete the tenant's last two workspaces cannot both see a spare one and proceed: a plain `DELETE ... WHERE EXISTS (another workspace)` reads a snapshot each transaction takes independently, which is exactly the race this avoids.
///
/// `conn` is a `&DatabaseTransaction` for the same reason `create_workspace`'s is: the advisory lock this takes is transaction-scoped, so handed a pool it would be released before the count and delete it guards, and the signature is what stops a caller from passing one.
pub(crate) async fn delete_workspace(
    conn: &DatabaseTransaction,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::workspace_workspaces;

    let workspace = workspace_workspaces::Entity::find_by_id(workspace_id)
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| {
            YorishiroError::not_found(format!("workspace '{workspace_id}' was not found"))
        })?;

    crate::db::lock_for_update(conn, &workspace_count_lock_key(workspace.tenant_id))
        .await
        .internal()?;

    let remaining = workspace_workspaces::Entity::find()
        .filter(workspace_workspaces::Column::TenantId.eq(workspace.tenant_id))
        .count(conn)
        .await
        .internal()?;
    if remaining <= 1 {
        return Err(YorishiroError::Conflict {
            message: "cannot delete a tenant's only remaining workspace".into(),
        });
    }

    workspace_workspaces::Entity::delete_by_id(workspace_id)
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}
