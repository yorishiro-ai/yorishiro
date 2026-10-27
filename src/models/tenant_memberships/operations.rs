//! Tenant membership and role operations.

use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{tenant_memberships::Column, user_users};

use super::{ActiveModel, Entity, MembershipRecord, MembershipRole};

/// Adds (or updates the role of) a user's membership in a tenant.
///
/// Takes `&impl ConnectionTrait` so a caller can compose this with `create_user` in one transaction, same reasoning as `create_user`'s doc comment.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn add_member(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<(), YorishiroError> {
    use sea_orm::sea_query::OnConflict;

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
pub(crate) async fn update_member_access(
    db: &sea_orm::DatabaseConnection,
    tenant_id: Uuid,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<Vec<super::MembershipKeyAccess>, YorishiroError> {
    use crate::models::_entities::api_keys;
    let txn = crate::db::begin_write(db).await.internal()?;
    add_member(&txn, tenant_id, user_id, role).await?;
    let keys = api_keys::Entity::find()
        .filter(api_keys::Column::TenantId.eq(tenant_id))
        .filter(api_keys::Column::UserId.eq(user_id))
        .order_by_asc(api_keys::Column::Id)
        .all(&txn)
        .await
        .internal()?;
    let access = keys
        .into_iter()
        .map(|key| {
            let stored_scope = crate::models::api_keys::ApiKeyScope::from_db_str(&key.scope)
                .ok_or_else(|| {
                    YorishiroError::Internal(anyhow::anyhow!("unknown api key scope"))
                })?;
            Ok(super::MembershipKeyAccess {
                api_key_id: key.id,
                stored_scope,
                effective_scope: stored_scope.min(role.max_scope()),
                audit: key.audit,
            })
        })
        .collect::<Result<Vec<_>, YorishiroError>>()?;
    txn.commit().await.internal()?;
    Ok(access)
}

/// Removes membership; subsequent authentication rejects all attributed keys in this tenant.
/// Revokes attributed keys atomically so restoring membership cannot resurrect old credentials.
///
/// # Errors
/// Returns an error when the membership is missing or persistence fails.
pub async fn remove_member(
    db: &sea_orm::DatabaseConnection,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::api_keys;
    let txn = crate::db::begin_write(db).await.internal()?;
    let result = Entity::delete_many()
        .filter(Column::TenantId.eq(tenant_id))
        .filter(Column::UserId.eq(user_id))
        .exec(&txn)
        .await
        .internal()?;
    if result.rows_affected == 0 {
        return Err(YorishiroError::not_found("tenant membership not found"));
    }
    api_keys::Entity::delete_many()
        .filter(api_keys::Column::TenantId.eq(tenant_id))
        .filter(api_keys::Column::UserId.eq(user_id))
        .exec(&txn)
        .await
        .internal()?;
    txn.commit().await.internal()?;
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
