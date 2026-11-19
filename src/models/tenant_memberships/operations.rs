//! Tenant membership operations: adding and removing members, creating tenant-bound users, and previewing the effect of a removal.

use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{tenant_memberships::Column, user_users};
use crate::models::api_keys::ApiKeyScope;
use crate::models::iam::policy::{Action, Resource};

use super::{ActiveModel, Entity, MembershipRecord};

async fn protect_root(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), YorishiroError> {
    crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
        .await
        .internal()?;
    if crate::models::iam_tenant_roots::Model::contains(txn, tenant_id, user_id).await? {
        return Err(YorishiroError::Conflict {
            message: "the tenant root cannot be changed or removed".into(),
        });
    }
    Ok(())
}

/// Reports membership removal effects without changing membership or credentials.
///
/// # Errors
/// Rejects stale credentials, removing the tenant root, or persistence failures.
pub async fn preview_removal(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    user_id: Uuid,
) -> Result<super::MembershipRemovalPreview, YorishiroError> {
    use crate::models::_entities::{api_keys, workspace_workspaces};
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        ApiKeyScope::Read,
        Action::TenantUserDelete,
        Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    let member = is_member(&txn, actor.tenant_id, user_id).await?;
    protect_root(&txn, actor.tenant_id, user_id).await?;
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
    let lost_workspace_ids = if member {
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
        member,
        revoked_api_key_ids,
        lost_workspace_ids,
    })
}

/// # Errors
/// Rejects changing the tenant root, accounts of another tenant, or persistence failures.
pub async fn add_member(
    db: &sea_orm::DatabaseConnection,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    add_member_in_transaction(&txn, tenant_id, user_id).await?;
    txn.commit().await.internal()?;
    Ok(())
}

pub(crate) async fn add_member_in_transaction(
    conn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), YorishiroError> {
    use sea_orm::sea_query::OnConflict;

    crate::db::lock_for_update(conn, &format!("tenant-membership:{tenant_id}"))
        .await
        .internal()?;
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
        ..Default::default()
    };
    Entity::insert(active)
        .on_conflict(OnConflict::column(Column::UserId).do_nothing().to_owned())
        .exec_without_returning(conn)
        .await
        .internal()?;
    Ok(())
}

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
) -> Result<user_users::Model, YorishiroError> {
    let user = crate::models::user_users::create_tenant_user(
        txn,
        tenant_id,
        user_name,
        password,
        display_name,
    )
    .await?;
    add_member_in_transaction(txn, tenant_id, user.id).await?;
    Ok(user)
}

/// # Errors
/// Rejects unauthorized credentials, invalid credentials and duplicate names.
pub(crate) async fn create_tenant_user_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    input: &crate::dtos::members::CreateTenantUserRequest,
) -> Result<MembershipRecord, YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
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
    )
    .await?;
    txn.commit().await.internal()?;
    Ok(MembershipRecord {
        user_id: user.id,
        email: None,
        user_name: user.user_name,
        display_name: user.display_name,
    })
}

/// # Errors
/// Rejects unauthorized credentials, root recovery and users outside the tenant.
pub async fn reset_tenant_user_password(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    user_id: Uuid,
    password: &str,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
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

/// # Errors
/// Rejects revoked credentials, unauthorized callers, removing the tenant root and persistence failures.
pub async fn remove_member_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    user_id: Uuid,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
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
    protect_root(txn, tenant_id, user_id).await?;
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

pub(crate) async fn list_members_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<MembershipRecord>, YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        ApiKeyScope::Read,
        Action::TenantUserList,
        Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    list_members(&txn, actor.tenant_id, page).await
}

/// Lists a tenant's members, excluding its root, one page at a time.
///
/// # Errors
/// Returns an error if the query fails.
pub async fn list_members(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<MembershipRecord>, YorishiroError> {
    let mut query = Entity::find().filter(Column::TenantId.eq(tenant_id));
    if let Some(root) = crate::models::iam_tenant_roots::Entity::find_by_id(tenant_id)
        .one(conn)
        .await
        .internal()?
    {
        query = query.filter(Column::UserId.ne(root.user_id));
    }
    let memberships = query
        .find_also_related(user_users::Entity)
        .order_by_asc(Column::CreatedAt)
        .order_by_asc(Column::Id)
        .limit(page.limit() as u64)
        .offset(page.offset() as u64)
        .all(conn)
        .await
        .internal()?;

    Ok(memberships
        .into_iter()
        .filter_map(|(_, user)| {
            let user = user?;
            Some(MembershipRecord {
                user_id: user.id,
                email: user.email,
                user_name: user.user_name,
                display_name: user.display_name,
            })
        })
        .collect())
}

pub(crate) async fn is_member(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<bool, YorishiroError> {
    Ok(Entity::find()
        .filter(Column::TenantId.eq(tenant_id))
        .filter(Column::UserId.eq(user_id))
        .one(conn)
        .await
        .internal()?
        .is_some())
}
