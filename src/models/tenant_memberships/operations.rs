//! Tenant membership and role operations.

use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect,
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
/// Rejects stale administrator credentials, removing the tenant root, or persistence failures.
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
    protect_root(&txn, actor.tenant_id, user_id, None).await?;
    let last_admin_warning = if is_last_admin(&txn, actor.tenant_id, user_id).await? {
        Some(LAST_ADMIN_WARNING.to_owned())
    } else {
        None
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
        last_admin_warning,
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

/// Serializes membership changes and refuses removing or demoting the tenant root.
async fn protect_root(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
    next_role: Option<MembershipRole>,
) -> Result<(), YorishiroError> {
    crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
        .await
        .internal()?;
    if next_role != Some(MembershipRole::Admin)
        && crate::models::iam_tenant_roots::Model::contains(txn, tenant_id, user_id).await?
    {
        return Err(YorishiroError::Conflict {
            message: "cannot remove or demote the tenant root".into(),
        });
    }
    Ok(())
}

/// Carried in role and removal previews when the change leaves no non-root admin behind.
pub(crate) const LAST_ADMIN_WARNING: &str = "this leaves no non-root admin; only the tenant root retains full access and can restore administrator membership";

/// True when `user_id` holds the admin role and no other non-root admin membership exists in the tenant.
/// The tenant root keeps its admin membership no matter what happens to other members, so it cannot count as a surviving admin.
async fn is_last_admin(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<bool, YorishiroError> {
    if get_membership_role(txn, tenant_id, user_id).await? != Some(MembershipRole::Admin) {
        return Ok(false);
    }
    let root = crate::models::iam_tenant_roots::Entity::find_by_id(tenant_id)
        .one(txn)
        .await
        .internal()?;
    let mut query = Entity::find()
        .filter(Column::TenantId.eq(tenant_id))
        .filter(Column::Role.eq(MembershipRole::Admin.as_db_str()))
        .filter(Column::UserId.ne(user_id));
    if let Some(root) = root {
        query = query.filter(Column::UserId.ne(root.user_id));
    }
    let others = query
        .select_only()
        .column(Column::Id)
        .into_tuple::<Uuid>()
        .all(txn)
        .await
        .internal()?;
    Ok(others.is_empty())
}

/// Adds (or updates the role of) a user's membership in a tenant.
///
/// # Errors
/// Rejects removing or demoting the tenant root, or persistence failures.
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

    protect_root(conn, tenant_id, user_id, Some(role)).await?;
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
    guard_tenant_bound_user(conn, tenant_id, user_id).await?;

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
    crate::models::iam_builtin::sync_admin_group(conn, tenant_id, user_id, Some(role)).await?;

    Ok(())
}

/// A tenant-bound user belongs to its own tenant only.
async fn guard_tenant_bound_user(
    conn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), YorishiroError> {
    let Some(user) = user_users::Entity::find_by_id(user_id)
        .one(conn)
        .await
        .internal()?
    else {
        return Err(YorishiroError::not_found("account not found"));
    };
    if let Some(bound) = user.tenant_id
        && bound != tenant_id
    {
        return Err(YorishiroError::Conflict {
            message: "a tenant user cannot join another tenant".into(),
        });
    }
    Ok(())
}

/// Creates a tenant-bound user and its membership in the caller's transaction.
///
/// # Errors
/// Rejects invalid credentials, and duplicate names.
pub async fn create_tenant_user_member(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_name: &str,
    password: &str,
    display_name: Option<&str>,
    role: MembershipRole,
) -> Result<user_users::Model, YorishiroError> {
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

/// Creates a tenant user after checking the caller's IAM permission.
///
/// # Errors
/// Rejects unauthorized credentials, invalid credentials and duplicate names.
pub(crate) async fn create_tenant_user_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    input: &crate::dtos::members::CreateTenantUserRequest,
) -> Result<MembershipRecord, YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::access::require_iam_action(
        &txn,
        actor,
        ApiKeyScope::Read,
        Action::TenantUserCreate,
        Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    let user = create_tenant_user_member(
        &txn,
        actor.tenant_id,
        &input.user_name,
        &input.password,
        input.display_name.as_deref(),
        input.role,
    )
    .await?;
    txn.commit().await.internal()?;
    Ok(MembershipRecord {
        user_id: user.id,
        email: None,
        user_name: user.user_name,
        display_name: user.display_name,
        role: input.role,
    })
}

/// Resets a tenant user's password with IAM authorization.
///
/// # Errors
/// Rejects unauthorized credentials, root recovery and users outside the tenant.
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
/// Rejects stale administrator credentials, removing or demoting the tenant root, and persistence failures.
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
    add_member_in_transaction(txn, tenant_id, user_id, role).await?;
    key_access(txn, tenant_id, user_id).await
}

/// Previews a role change without updating membership or stored grants.
///
/// # Errors
/// Rejects stale administrator credentials, demoting the tenant root, missing membership or persistence failures.
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
    protect_root(&txn, actor.tenant_id, user_id, Some(role)).await?;
    let last_admin_warning =
        if role != MembershipRole::Admin && is_last_admin(&txn, actor.tenant_id, user_id).await? {
            Some(LAST_ADMIN_WARNING.to_owned())
        } else {
            None
        };
    let keys = key_access(&txn, actor.tenant_id, user_id).await?;
    // Membership labels do not change IAM grants or credential ceilings.
    let lost_workspace_ids = Vec::new();
    txn.rollback().await.internal()?;
    Ok(super::MembershipRolePreview {
        user_id,
        current_role,
        requested_role: role,
        last_admin_warning,
        lost_workspace_ids,
        keys,
    })
}

async fn key_access(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
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
        let effective_scope = Some(stored_scope);
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
/// Rejects removing the tenant root or persistence failures.
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
/// Rejects revoked credentials, non-administrators, removing the tenant root and persistence failures.
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
    protect_root(txn, tenant_id, user_id, None).await?;
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
