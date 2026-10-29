//! Tenant membership and role operations.

use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{tenant_memberships::Column, user_users};

use super::{ActiveModel, Entity, MembershipRecord, MembershipRole};

/// Reports membership removal effects without changing membership or credentials.
///
/// # Errors
/// Rejects stale administrator credentials or persistence failures.
pub async fn preview_removal(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    user_id: Uuid,
) -> Result<super::MembershipRemovalPreview, YorishiroError> {
    use crate::models::_entities::{api_keys, workspace_workspaces};
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::access::require_current_tenant_admin(&txn, actor).await?;
    let role = get_membership_role(&txn, actor.tenant_id, user_id).await?;
    let blocked_by_last_owner = if role == Some(MembershipRole::Owner) {
        Entity::find()
            .filter(Column::TenantId.eq(actor.tenant_id))
            .filter(Column::Role.eq(MembershipRole::Owner.as_db_str()))
            .count(&txn)
            .await
            .internal()?
            <= 1
    } else {
        false
    };
    let revoked_api_key_ids = api_keys::Entity::find()
        .filter(api_keys::Column::TenantId.eq(actor.tenant_id))
        .filter(api_keys::Column::UserId.eq(user_id))
        .select_only()
        .column(api_keys::Column::Id)
        .order_by_asc(api_keys::Column::Id)
        .into_tuple::<Uuid>()
        .all(&txn)
        .await
        .internal()?;
    let lost_workspace_ids = if role.is_some() {
        let candidates = crate::models::tenancy::list_workspaces_for_user(&txn, user_id).await?;
        workspace_workspaces::Entity::find()
            .filter(workspace_workspaces::Column::TenantId.eq(actor.tenant_id))
            .filter(
                workspace_workspaces::Column::Id
                    .is_in(candidates.into_iter().map(|workspace| workspace.id)),
            )
            .select_only()
            .column(workspace_workspaces::Column::Id)
            .order_by_asc(workspace_workspaces::Column::Id)
            .into_tuple::<Uuid>()
            .all(&txn)
            .await
            .internal()?
    } else {
        Vec::new()
    };
    txn.rollback().await.internal()?;
    Ok(super::MembershipRemovalPreview {
        user_id,
        role,
        blocked_by_last_owner,
        revoked_api_key_ids,
        lost_workspace_ids,
    })
}

/// Transfers ownership to an existing member without leaving the tenant ownerless.
///
/// # Errors
/// Rejects stale credentials, non-owner actors, self-transfer and missing recipients.
pub async fn transfer_ownership(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    successor_id: Uuid,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::access::require_current_tenant_admin(&txn, actor).await?;
    let user_id = actor.user_id.ok_or(YorishiroError::Unauthenticated)?;
    if get_membership_role(&txn, actor.tenant_id, user_id).await? != Some(MembershipRole::Owner) {
        return Err(YorishiroError::ScopeInsufficient {
            message: "only a tenant owner can transfer ownership".into(),
            hint: "ask a current tenant owner to perform the transfer".into(),
        });
    }
    if user_id == successor_id {
        return Err(YorishiroError::Conflict {
            message: "ownership must be transferred to another member".into(),
        });
    }
    get_membership_role(&txn, actor.tenant_id, successor_id)
        .await?
        .ok_or_else(|| YorishiroError::not_found("successor tenant membership not found"))?;
    add_member_in_transaction(&txn, actor.tenant_id, successor_id, MembershipRole::Owner).await?;
    add_member_in_transaction(&txn, actor.tenant_id, user_id, MembershipRole::Admin).await?;
    txn.commit().await.internal()?;
    Ok(())
}

/// Serializes membership changes and refuses losing the last owner.
async fn protect_owner(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
    next_role: Option<MembershipRole>,
) -> Result<(), YorishiroError> {
    crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
        .await
        .internal()?;
    if next_role == Some(MembershipRole::Owner)
        || get_membership_role(txn, tenant_id, user_id).await? != Some(MembershipRole::Owner)
    {
        return Ok(());
    }
    let owners = Entity::find()
        .filter(Column::TenantId.eq(tenant_id))
        .filter(Column::Role.eq(MembershipRole::Owner.as_db_str()))
        .count(txn)
        .await
        .internal()?;
    if owners <= 1 {
        return Err(YorishiroError::Conflict {
            message: "cannot remove or demote the tenant's last owner".into(),
        });
    }
    Ok(())
}

/// Adds (or updates the role of) a user's membership in a tenant.
///
/// # Errors
/// Rejects last-owner demotion or persistence failures.
pub async fn add_member(
    db: &sea_orm::DatabaseConnection,
    tenant_id: Uuid,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    add_member_in_transaction(&txn, tenant_id, user_id, role).await?;
    txn.commit().await.internal()?;
    Ok(())
}

pub(crate) async fn add_member_in_transaction(
    conn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<(), YorishiroError> {
    use sea_orm::sea_query::OnConflict;

    protect_owner(conn, tenant_id, user_id, Some(role)).await?;

    // `Entity::insert(...).on_conflict(...).exec(...)` builds its query eagerly from `active` and never calls `ActiveModelBehavior::before_save`, unlike plain `ActiveModel::insert()`: this is the one insert path in this file that needs `sqlite_generated_id` called directly rather than relying on the hook.
    let active = ActiveModel {
        id: crate::db::sqlite_generated_id(conn, ActiveValue::NotSet),
        tenant_id: ActiveValue::Set(tenant_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(role.as_db_str().to_string()),
        ..Default::default()
    };

    Entity::insert(active)
        .on_conflict(
            OnConflict::columns([Column::TenantId, Column::UserId])
                .update_column(Column::Role)
                .to_owned(),
        )
        .exec(conn)
        .await
        .internal()?;

    Ok(())
}

/// Updates membership and reports existing key grants in the same transaction.
///
/// # Errors
/// Rejects stale administrator credentials, last-owner demotion and persistence failures.
pub async fn update_member_access(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<Vec<super::MembershipKeyAccess>, YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::access::require_current_tenant_admin(&txn, actor).await?;
    let tenant_id = actor.tenant_id;
    if role == MembershipRole::Owner
        || get_membership_role(&txn, tenant_id, user_id).await? == Some(MembershipRole::Owner)
    {
        crate::models::access::require_current_tenant_owner(&txn, actor).await?;
    }
    add_member_in_transaction(&txn, tenant_id, user_id, role).await?;
    let access = key_access(&txn, tenant_id, user_id, role).await?;
    txn.commit().await.internal()?;
    Ok(access)
}

/// Previews a role change without updating membership or stored grants.
///
/// # Errors
/// Rejects stale administrator credentials, missing membership or persistence failures.
pub async fn preview_role_change(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<super::MembershipRolePreview, YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::access::require_current_tenant_admin(&txn, actor).await?;
    let current_role = get_membership_role(&txn, actor.tenant_id, user_id)
        .await?
        .ok_or_else(|| YorishiroError::not_found("tenant membership not found"))?;
    let blocked_by_last_owner = if role != MembershipRole::Owner {
        match protect_owner(&txn, actor.tenant_id, user_id, Some(role)).await {
            Ok(()) => false,
            Err(YorishiroError::Conflict { .. }) => true,
            Err(error) => return Err(error),
        }
    } else {
        false
    };
    let keys = key_access(&txn, actor.tenant_id, user_id, role).await?;
    let mut lost_workspace_ids = Vec::new();
    let candidates = crate::models::tenancy::list_workspaces_for_user(&txn, user_id).await?;
    use crate::models::_entities::workspace_workspaces;
    let workspace_ids = workspace_workspaces::Entity::find()
        .filter(workspace_workspaces::Column::TenantId.eq(actor.tenant_id))
        .filter(workspace_workspaces::Column::Id.is_in(candidates.into_iter().map(|w| w.id)))
        .select_only()
        .column(workspace_workspaces::Column::Id)
        .into_tuple::<Uuid>()
        .all(&txn)
        .await
        .internal()?;
    for workspace_id in workspace_ids {
        if crate::models::access::workspace_access_for_role(&txn, workspace_id, user_id, role)
            .await?
            .is_none()
        {
            lost_workspace_ids.push(workspace_id);
        }
    }
    lost_workspace_ids.sort_unstable();
    txn.rollback().await.internal()?;
    Ok(super::MembershipRolePreview {
        user_id,
        current_role,
        requested_role: role,
        blocked_by_last_owner,
        lost_workspace_ids,
        keys,
    })
}

async fn key_access(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<Vec<super::MembershipKeyAccess>, YorishiroError> {
    use crate::models::_entities::api_keys;
    let keys = api_keys::Entity::find()
        .filter(api_keys::Column::TenantId.eq(tenant_id))
        .filter(api_keys::Column::UserId.eq(user_id))
        .order_by_asc(api_keys::Column::Id)
        .all(txn)
        .await
        .internal()?;
    let mut access = Vec::with_capacity(keys.len());
    for key in keys {
        let stored_scope = crate::models::api_keys::ApiKeyScope::from_db_str(&key.scope)
            .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("unknown api key scope")))?;
        let effective_scope = if let Some(workspace_id) = key.workspace_id {
            crate::models::access::workspace_access_for_role(txn, workspace_id, user_id, role)
                .await?
                .map(|access| stored_scope.min(access.max_scope))
        } else {
            None
        };
        access.push(super::MembershipKeyAccess {
            api_key_id: key.id,
            stored_scope,
            effective_scope,
            audit: key.audit,
        });
    }
    Ok(access)
}

/// Removes membership; subsequent authentication rejects all attributed keys in this tenant.
/// Revokes attributed keys atomically so restoring membership cannot resurrect old credentials.
///
/// # Errors
/// Rejects last-owner removal or persistence failures.
pub async fn remove_member(
    db: &sea_orm::DatabaseConnection,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    remove_member_in_transaction(&txn, tenant_id, user_id).await?;
    txn.commit().await.internal()?;
    Ok(())
}

/// Removes a member only if the presented administrator is still authorized.
///
/// # Errors
/// Rejects revoked credentials, non-administrators, last-owner removal and persistence failures.
pub async fn remove_member_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    user_id: Uuid,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::access::require_current_tenant_admin(&txn, actor).await?;
    if get_membership_role(&txn, actor.tenant_id, user_id).await? == Some(MembershipRole::Owner) {
        crate::models::access::require_current_tenant_owner(&txn, actor).await?;
    }
    remove_member_in_transaction(&txn, actor.tenant_id, user_id).await?;
    txn.commit().await.internal()?;
    Ok(())
}

async fn remove_member_in_transaction(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::{api_keys, workspace_memberships, workspace_workspaces};
    use sea_orm::QueryTrait;
    protect_owner(txn, tenant_id, user_id, None).await?;
    let workspaces = workspace_workspaces::Entity::find()
        .select_only()
        .column(workspace_workspaces::Column::Id)
        .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
        .into_query();
    workspace_memberships::Entity::delete_many()
        .filter(workspace_memberships::Column::UserId.eq(user_id))
        .filter(workspace_memberships::Column::WorkspaceId.in_subquery(workspaces))
        .exec(txn)
        .await
        .internal()?;
    Entity::delete_many()
        .filter(Column::TenantId.eq(tenant_id))
        .filter(Column::UserId.eq(user_id))
        .exec(txn)
        .await
        .internal()?;
    api_keys::Entity::delete_many()
        .filter(api_keys::Column::TenantId.eq(tenant_id))
        .filter(api_keys::Column::UserId.eq(user_id))
        .exec(txn)
        .await
        .internal()?;
    Ok(())
}

/// Every member of a tenant, joined against their user row.
pub(crate) async fn list_members(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<MembershipRecord>, YorishiroError> {
    let memberships = Entity::find()
        .filter(Column::TenantId.eq(tenant_id))
        .find_also_related(user_users::Entity)
        .order_by_asc(Column::CreatedAt)
        .limit(page.limit() as u64)
        .offset(page.offset() as u64)
        .all(conn)
        .await
        .internal()?;

    Ok(memberships
        .into_iter()
        .filter_map(|(membership, user)| {
            let user = user?;
            let role = MembershipRole::from_db_str(&membership.role)?;
            Some(MembershipRecord {
                user_id: user.id,
                email: user.email,
                display_name: user.display_name,
                role,
            })
        })
        .collect())
}

/// Looks up a single user's role within a tenant, or `None` if they aren't a member.
pub(crate) async fn get_membership_role(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<MembershipRole>, YorishiroError> {
    let membership = Entity::find()
        .filter(Column::TenantId.eq(tenant_id))
        .filter(Column::UserId.eq(user_id))
        .one(conn)
        .await
        .internal()?;

    Ok(membership.and_then(|m| MembershipRole::from_db_str(&m.role)))
}
