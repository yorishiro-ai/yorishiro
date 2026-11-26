//! Tenant membership operations: adding and removing members, creating tenant-bound users and listing them.

use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{tenant_memberships::Column, user_users};

use super::{ActiveModel, Entity, MembershipRecord};

pub(crate) async fn protect_root(
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

    // `Entity::insert(...).on_conflict(...).exec(...)` builds its query eagerly from `active` and never calls `ActiveModelBehavior::before_save`, unlike plain `ActiveModel::insert()`: this is the one insert path in this file that needs `generated_id` called directly rather than relying on the hook.
    let active = ActiveModel {
        id: crate::db::generated_id(ActiveValue::NotSet),
        tenant_id: ActiveValue::Set(tenant_id),
        user_id: ActiveValue::Set(user_id),
        ..Default::default()
    };
    let active = sea_orm::ActiveModelBehavior::before_save(active, conn, true)
        .await
        .internal()?;
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

pub(crate) async fn remove_member_in_transaction(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::access_keys;
    protect_root(txn, tenant_id, user_id).await?;
    Entity::delete_many()
        .filter(Column::TenantId.eq(tenant_id))
        .filter(Column::UserId.eq(user_id))
        .exec(txn)
        .await
        .internal()?;
    access_keys::Entity::revoke_matching(
        txn,
        sea_orm::Condition::all()
            .add(access_keys::Column::TenantId.eq(tenant_id))
            .add(access_keys::Column::UserId.eq(user_id)),
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

/// Lists a tenant's members, excluding its root, one page at a time.
///
/// # Errors
/// Returns an error if the query fails.
pub async fn list_members(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<MembershipRecord>, YorishiroError> {
    Ok(list_member_users(conn, tenant_id, page)
        .await?
        .into_iter()
        .map(|user| MembershipRecord {
            user_id: user.id,
            email: user.email,
            user_name: user.user_name,
            display_name: user.display_name,
        })
        .collect())
}

/// Lists the users of a tenant, excluding its root, one page at a time.
///
/// # Errors
/// Returns an error if the query fails.
pub(crate) async fn list_member_users(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<user_users::Model>, YorishiroError> {
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
        .limit(page.limit())
        .offset(page.offset())
        .all(conn)
        .await
        .internal()?;
    Ok(memberships
        .into_iter()
        .filter_map(|(_, user)| user)
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
