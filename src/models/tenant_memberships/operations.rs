//! Tenant membership and role operations.

use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{tenant_memberships::Column, user_users};
use crate::models::api_keys::ApiKeyScope;
use crate::models::iam_policy::{Action, Resource};

use super::{ActiveModel, Entity, MembershipRecord, MembershipRole};

pub(crate) async fn update_member_by_email_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    email: &str,
    role: MembershipRole,
) -> Result<
    (
        crate::models::user_users::Model,
        Vec<super::MembershipKeyAccess>,
    ),
    YorishiroError,
> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::access::require_iam_action(
        &txn,
        actor,
        ApiKeyScope::Read,
        Action::TenantUserUpdate,
        Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    let user = crate::models::user_users::get_user_by_email(&txn, email)
        .await?
        .ok_or_else(|| YorishiroError::not_found("account not found"))?;
    let keys = update_member_access_on(&txn, actor, user.id, role).await?;
    txn.commit().await.internal()?;
    Ok((user, keys))
}

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
    crate::models::access::require_iam_action(
        &txn,
        actor,
        ApiKeyScope::Read,
        Action::TenantUserDelete,
        Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
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
        .filter(api_keys::Column::RevokedAt.is_null())
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

/// Rejects ownership transfer because tenant root identity is immutable.
///
/// # Errors
/// Rejects unauthorized credentials or returns a conflict for authorized callers.
pub async fn transfer_ownership(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    _successor_id: Uuid,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::access::require_iam_action(
        &txn,
        actor,
        ApiKeyScope::Read,
        Action::TenantUserUpdate,
        Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    Err(YorishiroError::Conflict {
        message: "tenant root ownership cannot be transferred".into(),
    })
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
    if next_role != Some(MembershipRole::Owner)
        && crate::models::iam_tenant_roots::Model::contains(txn, tenant_id, user_id).await?
    {
        return Err(YorishiroError::Conflict {
            message: "cannot remove or demote the tenant root".into(),
        });
    }
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
    crate::db::lock_for_update(conn, &format!("user-membership:{user_id}"))
        .await
        .internal()?;
    if Entity::find()
        .filter(Column::UserId.eq(user_id))
        .filter(Column::TenantId.ne(tenant_id))
        .one(conn)
        .await
        .internal()?
        .is_some()
    {
        return Err(YorishiroError::Conflict {
            message: "an account cannot belong to more than one tenant".into(),
        });
    }
    guard_tenant_bound_user(conn, tenant_id, user_id, role).await?;

    // `Entity::insert(...).on_conflict(...).exec(...)` builds its query eagerly from `active` and never calls `ActiveModelBehavior::before_save`, unlike plain `ActiveModel::insert()`: this is the one insert path in this file that needs `sqlite_generated_id` called directly rather than relying on the hook.
    let active = ActiveModel {
        id: crate::db::sqlite_generated_id(conn, ActiveValue::NotSet),
        tenant_id: ActiveValue::Set(tenant_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(role.as_db_str().to_string()),
        ..Default::default()
    };

    let affected = Entity::insert(active)
        .on_conflict(
            OnConflict::column(Column::UserId)
                .update_column(Column::Role)
                .action_and_where(Column::TenantId.eq(tenant_id))
                .to_owned(),
        )
        .exec_without_returning(conn)
        .await
        .internal()?;
    if affected == 0 {
        return Err(YorishiroError::Conflict {
            message: "an account cannot belong to more than one tenant".into(),
        });
    }

    Ok(())
}

/// A tenant-bound user belongs to its own tenant only and can never own it.
async fn guard_tenant_bound_user(
    conn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<(), YorishiroError> {
    let Some(user) = user_users::Entity::find_by_id(user_id)
        .one(conn)
        .await
        .internal()?
    else {
        return Ok(());
    };
    match user.tenant_id {
        Some(bound) if bound != tenant_id => Err(YorishiroError::Conflict {
            message: "a tenant user cannot join another tenant".into(),
        }),
        Some(_) if role == MembershipRole::Owner => Err(YorishiroError::Conflict {
            message: "a tenant user cannot be an owner".into(),
        }),
        _ => Ok(()),
    }
}

/// Creates a tenant-bound user and its membership in the caller's transaction.
///
/// # Errors
/// Rejects owner role, invalid credentials, and duplicate names.
pub async fn create_tenant_user_member(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_name: &str,
    password: &str,
    display_name: Option<&str>,
    role: MembershipRole,
) -> Result<user_users::Model, YorishiroError> {
    if role == MembershipRole::Owner {
        return Err(YorishiroError::Conflict {
            message: "a tenant user cannot be an owner".into(),
        });
    }
    let user = crate::models::user_users::create_tenant_user(
        txn,
        tenant_id,
        user_name,
        password,
        display_name,
    )
    .await?;
    add_member_in_transaction(txn, tenant_id, user.id, role).await?;
    Ok(user)
}

/// Resets a tenant user's password as a current administrator.
///
/// # Errors
/// Rejects stale credentials; only an owner may reset an administrator's password.
pub async fn reset_tenant_user_password(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    user_id: Uuid,
    password: &str,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::access::require_iam_action(
        &txn,
        actor,
        ApiKeyScope::Read,
        Action::TenantUserPasswordReset,
        Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    if crate::models::iam_tenant_roots::Model::contains(&txn, actor.tenant_id, user_id).await? {
        return Err(YorishiroError::Conflict {
            message: "tenant root password recovery requires the administrator CLI".into(),
        });
    }
    crate::models::user_users::reset_tenant_password(&txn, actor.tenant_id, user_id, password)
        .await?;
    txn.commit().await.internal()?;
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
    let access = update_member_access_on(&txn, actor, user_id, role).await?;
    txn.commit().await.internal()?;
    Ok(access)
}

async fn update_member_access_on(
    txn: &DatabaseTransaction,
    actor: &crate::models::api_keys::AuthContext,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<Vec<super::MembershipKeyAccess>, YorishiroError> {
    crate::models::access::require_iam_action(
        txn,
        actor,
        ApiKeyScope::Read,
        Action::TenantUserUpdate,
        Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    let tenant_id = actor.tenant_id;
    if role == MembershipRole::Owner
        && !crate::models::iam_tenant_roots::Model::contains(txn, tenant_id, user_id).await?
    {
        return Err(YorishiroError::Conflict {
            message: "tenant root cannot be assigned through membership changes".into(),
        });
    }
    add_member_in_transaction(txn, tenant_id, user_id, role).await?;
    key_access(txn, tenant_id, user_id, role).await
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
    crate::models::access::require_iam_action(
        &txn,
        actor,
        ApiKeyScope::Read,
        Action::TenantUserUpdate,
        Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
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
        .filter(api_keys::Column::RevokedAt.is_null())
        .filter(
            sea_orm::Condition::any()
                .add(api_keys::Column::ExpiresAt.is_null())
                .add(api_keys::Column::ExpiresAt.gt(crate::time::now())),
        )
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
            Some(crate::models::access::effective_scope(stored_scope, role))
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
    crate::models::access::require_iam_action(
        &txn,
        actor,
        ApiKeyScope::Read,
        Action::TenantUserDelete,
        Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
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
    api_keys::Entity::revoke_matching(
        txn,
        sea_orm::Condition::all()
            .add(api_keys::Column::TenantId.eq(tenant_id))
            .add(api_keys::Column::UserId.eq(user_id)),
    )
    .await?;
    // A tenant-bound user has no life outside its tenant: removing the membership removes the account.
    user_users::Entity::delete_many()
        .filter(user_users::Column::Id.eq(user_id))
        .filter(user_users::Column::TenantId.eq(tenant_id))
        .exec(txn)
        .await
        .internal()?;
    Ok(())
}

/// Every member of a tenant, joined against their user row.
pub(crate) async fn list_members_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<MembershipRecord>, YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::access::require_iam_action(
        &txn,
        actor,
        ApiKeyScope::Read,
        Action::TenantUserList,
        Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    super::list_members(&txn, actor.tenant_id, page).await
}

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
                user_name: user.user_name,
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
