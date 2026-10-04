//! Explicit operations that compose more than one tenancy table.

use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait,
    PaginatorTrait, QueryFilter,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{tenant_memberships, workspace_workspaces};

use super::WorkspaceSummary;
use crate::models::workspace_workspaces::workspace_count_lock_key;

/// Every workspace `user_id` can log into: the union of workspaces under every tenant they hold a membership in.
/// Used by `/auth/login` to resolve `workspace_id` automatically when the caller can only reach one.
///
/// Deliberately unpaginated: this drives sign-in, not a browsing UI, and its own caller (`resolve_login_workspace`) needs the true, complete set to tell "exactly one, resolve to it" from "more than one, the caller must say which."
/// A default `LIMIT` here would silently hide a membership from a user who holds more workspaces than the page size, rather than list them.
pub(crate) async fn list_workspaces_for_user(
    conn: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<Vec<WorkspaceSummary>, YorishiroError> {
    use crate::models::_entities::workspace_workspaces;

    let memberships = tenant_memberships::Entity::find()
        .filter(tenant_memberships::Column::UserId.eq(user_id))
        .all(conn)
        .await
        .internal()?;

    let tenant_ids: Vec<Uuid> = memberships.into_iter().map(|m| m.tenant_id).collect();
    if tenant_ids.is_empty() {
        return Ok(vec![]);
    }

    let workspaces = workspace_workspaces::Entity::find()
        .filter(workspace_workspaces::Column::TenantId.is_in(tenant_ids))
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
/// Creates a workspace under `tenant_id`, enforcing the tenant's `max_workspaces` cap.
/// `None` means unlimited.
///
/// `conn` is a `&DatabaseTransaction` rather than a `&impl ConnectionTrait` because this takes `db::lock_for_update` before counting, and `pg_advisory_xact_lock` is transaction-scoped: handed a pool the lock would be released by the end of its own implicit transaction, before the count and insert it is meant to guard.
/// Taking the transaction in the signature makes passing a pool a compile error instead of a lock that silently does nothing.
/// It shares `delete_workspace`'s per-tenant lock key, so a create and a delete racing on the same tenant serialize against each other rather than each counting a total the other is about to change.
pub(crate) async fn create_workspace(
    conn: &DatabaseTransaction,
    tenant_id: Uuid,
    name: &str,
    max_entities: Option<i32>,
    schema_id: Option<Uuid>,
) -> Result<workspace_workspaces::Model, YorishiroError> {
    use crate::models::_entities::tenant_tenants;
    use crate::models::workspace_workspaces::WorkspaceStatus;

    let tenant = tenant_tenants::Entity::find_by_id(tenant_id)
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("tenant '{tenant_id}' was not found")))?;

    if let Some(max) = tenant.max_workspaces {
        crate::db::lock_for_update(conn, &workspace_count_lock_key(tenant_id))
            .await
            .internal()?;
        let count = workspace_workspaces::Entity::find()
            .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
            .count(conn)
            .await
            .internal()?;
        if count >= max as u64 {
            return Err(YorishiroError::Conflict {
                message: format!(
                    "tenant '{tenant_id}' has reached its workspace limit ({max}); \
                     raise max_workspaces or delete an existing workspace"
                ),
            });
        }
    }

    let active = workspace_workspaces::ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        name: ActiveValue::Set(name.to_string()),
        max_entities: ActiveValue::Set(max_entities),
        schema_id: ActiveValue::Set(schema_id),
        status: ActiveValue::Set(
            if schema_id.is_some() {
                WorkspaceStatus::Active.as_db_str()
            } else {
                WorkspaceStatus::SchemaPending.as_db_str()
            }
            .to_string(),
        ),
        ..Default::default()
    };

    active.insert(conn).await.internal()
}
