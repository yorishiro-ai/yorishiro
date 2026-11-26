//! User creation, self-service profiles and authentication lookups.

use crate::error::OptionExt;
use loco_rs::hash;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
};
use sea_orm::{Condition, QuerySelect};

use super::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::tenant_memberships as memberships;
use crate::models::_entities::user_users::Column;

/// # Errors
/// Rejects mismatched confirmation, weak passwords, non-root or passwordless accounts and missing, ambiguous or disabled tenant memberships.
/// Password and key changes roll back together if persistence fails.
pub async fn reset_root_password_by_operator(
    db: &sea_orm::DatabaseConnection,
    user_id: uuid::Uuid,
    confirmed_user_id: uuid::Uuid,
    password: &str,
) -> Result<(), YorishiroError> {
    if user_id != confirmed_user_id {
        return Err(YorishiroError::Conflict {
            message: "confirmed_user_id must match user_id".into(),
        });
    }
    if password.chars().count() < MIN_TENANT_PASSWORD_CHARS {
        return Err(YorishiroError::Conflict {
            message: format!(
                "password must contain at least {MIN_TENANT_PASSWORD_CHARS} characters"
            ),
        });
    }
    let txn = crate::db::begin_write(db).await.internal()?;
    let current = memberships::Entity::find()
        .filter(memberships::Column::UserId.eq(user_id))
        .all(&txn)
        .await
        .internal()?;
    let [membership] = current.as_slice() else {
        return Err(YorishiroError::Conflict {
            message: "the account must belong to exactly one tenant".into(),
        });
    };
    let tenant_id = membership.tenant_id;
    // Match login key issuance and membership mutation lock ordering, then recheck discovery.
    crate::db::lock_for_update(&txn, &format!("tenant-membership:{tenant_id}"))
        .await
        .internal()?;
    crate::db::lock_for_update(&txn, &format!("user-membership:{user_id}"))
        .await
        .internal()?;
    let current = memberships::Entity::find()
        .filter(memberships::Column::UserId.eq(user_id))
        .all(&txn)
        .await
        .internal()?;
    if current.len() != 1 || current[0].tenant_id != tenant_id {
        return Err(YorishiroError::Conflict {
            message: "account membership changed; retry the reset".into(),
        });
    }
    if !crate::models::iam_tenant_roots::Model::contains(&txn, tenant_id, user_id).await? {
        return Err(YorishiroError::Conflict {
            message: "password recovery requires the tenant root account".into(),
        });
    }
    crate::models::tenant_tenants::Model::require_active(&txn, tenant_id).await?;
    let user = Entity::find_by_id(user_id)
        .one(&txn)
        .await
        .internal()?
        .or_not_found("user not found")?;
    if user.tenant_id.is_some() || user.password_hash.is_none() {
        return Err(YorishiroError::Conflict {
            message: "password recovery requires an existing password-bearing root account".into(),
        });
    }
    let mut active: ActiveModel = user.into();
    active.password_hash = ActiveValue::Set(Some(hash_blocking(password).await?));
    active.update(&txn).await.internal()?;
    crate::models::access_keys::Entity::revoke_matching(
        &txn,
        Condition::all().add(crate::models::_entities::access_keys::Column::UserId.eq(user_id)),
    )
    .await?;
    txn.commit().await.internal()?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeletionOutcome {
    Deleted,
    TenantDeletionScheduled {
        tenant_id: uuid::Uuid,
        delete_scheduled_at: chrono::DateTime<chrono::FixedOffset>,
    },
    TenantDeleted {
        tenant_id: uuid::Uuid,
    },
}

/// Deletes an account with a trusted operator identity.
///
/// # Errors
/// Rejects a mismatched confirmation or a non-root account with a remaining membership.
pub async fn delete_by_operator(
    db: &sea_orm::DatabaseConnection,
    user_id: uuid::Uuid,
    confirmed_user_id: uuid::Uuid,
) -> Result<DeletionOutcome, YorishiroError> {
    if user_id != confirmed_user_id {
        return Err(YorishiroError::Conflict {
            message: "confirmed_user_id must match user_id".into(),
        });
    }
    let txn = crate::db::begin_write(db).await.internal()?;
    // Match login key issuance and membership mutation lock ordering: tenant fence, then user fence, before the row lock.
    let preliminary = memberships::Entity::find()
        .filter(memberships::Column::UserId.eq(user_id))
        .one(&txn)
        .await
        .internal()?;
    if let Some(preliminary) = &preliminary {
        crate::db::lock_for_update(
            &txn,
            &format!("tenant-membership:{}", preliminary.tenant_id),
        )
        .await
        .internal()?;
    }
    crate::db::lock_for_update(&txn, &format!("user-membership:{user_id}"))
        .await
        .internal()?;
    // The row lock also serializes concurrent membership/key inserts through their user foreign key.
    Entity::find_by_id(user_id)
        .lock_exclusive()
        .one(&txn)
        .await
        .internal()?
        .or_not_found("user not found")?;
    let membership = memberships::Entity::find()
        .filter(memberships::Column::UserId.eq(user_id))
        .one(&txn)
        .await
        .internal()?;
    let Some(membership) = membership else {
        // Revoke before ON DELETE SET NULL can detach user attribution from authenticating keys.
        crate::models::access_keys::Entity::revoke_matching(
            &txn,
            Condition::all().add(crate::models::_entities::access_keys::Column::UserId.eq(user_id)),
        )
        .await?;
        Entity::delete_by_id(user_id).exec(&txn).await.internal()?;
        txn.commit().await.internal()?;
        return Ok(DeletionOutcome::Deleted);
    };
    let tenant_id = membership.tenant_id;
    if !crate::models::iam_tenant_roots::Model::contains(&txn, tenant_id, user_id).await? {
        return Err(YorishiroError::Conflict {
            message: format!(
                "remove the user's membership in tenant {tenant_id} before deleting the account"
            ),
        });
    }
    // The root row references the membership row, so it comes off first; the cascade then removes the membership.
    crate::models::iam_tenant_roots::Entity::delete_by_id(tenant_id)
        .exec(&txn)
        .await
        .internal()?;
    let outcome =
        crate::models::tenant_tenants::retirement::root_deleted(&txn, tenant_id, user_id).await?;
    // Revoke before ON DELETE SET NULL can detach user attribution from authenticating keys.
    crate::models::access_keys::Entity::revoke_matching(
        &txn,
        Condition::all().add(crate::models::_entities::access_keys::Column::UserId.eq(user_id)),
    )
    .await?;
    Entity::delete_by_id(user_id).exec(&txn).await.internal()?;
    txn.commit().await.internal()?;
    Ok(outcome)
}

/// Changes the display name of a user of `tenant_id`, or clears it with `None`.
///
/// # Errors
/// Rejects an invalid display name and users outside the tenant.
pub(crate) async fn update_display_name(
    conn: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
    user_id: uuid::Uuid,
    display_name: Option<String>,
) -> Result<Model, YorishiroError> {
    let display_name = display_name.map(|name| name.trim().to_owned());
    if display_name
        .as_ref()
        .is_some_and(|name| name.is_empty() || name.chars().count() > 100)
    {
        return Err(YorishiroError::ValidationFailed {
            message: "display name must contain 1 to 100 characters, or be null".into(),
            details: vec![],
            hint: "provide a shorter display name or null to clear it".into(),
        });
    }
    let user = find_in_tenant(conn, tenant_id, user_id).await?;
    let mut active: ActiveModel = user.into();
    active.display_name = ActiveValue::Set(display_name);
    active.update(conn).await.internal()
}

/// Finds a user that belongs to `tenant_id`, whether bound to it or its root user.
///
/// # Errors
/// Returns not found for a user outside the tenant.
pub(crate) async fn find_in_tenant(
    conn: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<Model, YorishiroError> {
    if memberships::Entity::find()
        .filter(memberships::Column::TenantId.eq(tenant_id))
        .filter(memberships::Column::UserId.eq(user_id))
        .one(conn)
        .await
        .internal()?
        .is_none()
    {
        return Err(YorishiroError::not_found("user was not found"));
    }
    Entity::find_by_id(user_id)
        .one(conn)
        .await
        .internal()?
        .or_not_found("user was not found")
}

/// Creates a human user account.
/// The password is hashed with `loco_rs::hash` (Argon2id) before ever reaching the database.
///
/// Creates the user row only; `tenancy::register_member` composes account creation and membership in a caller-owned transaction.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn create_user(
    conn: &impl ConnectionTrait,
    email: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<Model, YorishiroError> {
    if password.chars().count() < MIN_TENANT_PASSWORD_CHARS {
        return Err(YorishiroError::ValidationFailed {
            message: format!(
                "password must contain at least {MIN_TENANT_PASSWORD_CHARS} characters"
            ),
            details: vec![],
            hint: "choose a longer password".into(),
        });
    }
    let password = password.to_owned();
    let password_hash = tokio::task::spawn_blocking(move || hash::hash_password(&password))
        .await
        .internal()?
        .internal()?;

    let active = ActiveModel {
        email: ActiveValue::Set(Some(email.to_string())),
        password_hash: ActiveValue::Set(Some(password_hash)),
        display_name: ActiveValue::Set(display_name.map(str::to_string)),
        ..Default::default()
    };

    active.insert(conn).await.map_err(|err| {
        YorishiroError::conflict_on_unique(
            err,
            format!("a user with email '{email}' already exists"),
        )
    })
}

/// Verifies an email/password pair against the stored Argon2id hash, returning the matching root user on success.
/// A passwordless account (`password_hash = NULL`) never matches, same as a wrong password: `loco_rs::hash::verify_password` needs a hash to compare against.
pub(crate) async fn verify_login(
    conn: &impl ConnectionTrait,
    email: &str,
    password: &str,
) -> Result<Option<Model>, YorishiroError> {
    let user = Entity::find()
        .filter(Column::Email.eq(email))
        .filter(Column::TenantId.is_null())
        .one(conn)
        .await
        .internal()?;
    check_password(user, password).await
}

/// Verifies a tenant-bound user's name and password within one tenant.
pub(crate) async fn verify_tenant_login(
    conn: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
    user_name: &str,
    password: &str,
) -> Result<Option<Model>, YorishiroError> {
    let user = Entity::find()
        .filter(Column::TenantId.eq(tenant_id))
        .filter(Column::UserNameKey.eq(crate::models::iam::names::name_key(user_name)))
        .one(conn)
        .await
        .internal()?;
    check_password(user, password).await
}

async fn check_password(
    user: Option<Model>,
    password: &str,
) -> Result<Option<Model>, YorishiroError> {
    let Some(user) = user else {
        return Ok(None);
    };
    let Some(password_hash) = user.password_hash.clone() else {
        return Ok(None);
    };
    let password = password.to_owned();
    let matches =
        tokio::task::spawn_blocking(move || hash::verify_password(&password, &password_hash))
            .await
            .internal()?;
    Ok(matches.then_some(user))
}

const MIN_TENANT_PASSWORD_CHARS: usize = 12;

fn validate_tenant_credentials(user_name: &str, password: &str) -> Result<(), YorishiroError> {
    crate::models::iam::names::validate_name(
        "user",
        user_name,
        crate::models::iam::names::MAX_USER_NAME_CHARS,
    )?;
    validate_password(password)
}

fn validate_password(password: &str) -> Result<(), YorishiroError> {
    if password.chars().count() < MIN_TENANT_PASSWORD_CHARS {
        return Err(YorishiroError::ValidationFailed {
            message: format!(
                "password must contain at least {MIN_TENANT_PASSWORD_CHARS} characters"
            ),
            details: vec![],
            hint: "choose a longer password".into(),
        });
    }
    Ok(())
}

async fn hash_blocking(password: &str) -> Result<String, YorishiroError> {
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || hash::hash_password(&password))
        .await
        .internal()?
        .internal()
}

/// Creates a user bound to one tenant, identified by `user_name` within it.
/// Creates the user row only; the caller adds the membership in the same transaction.
///
/// # Errors
/// Rejects an invalid name or password and a name already used in the tenant.
pub(crate) async fn create_tenant_user(
    conn: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
    user_name: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<Model, YorishiroError> {
    validate_tenant_credentials(user_name, password)?;
    let active = ActiveModel {
        tenant_id: ActiveValue::Set(Some(tenant_id)),
        user_name: ActiveValue::Set(Some(user_name.to_string())),
        user_name_key: ActiveValue::Set(Some(crate::models::iam::names::name_key(user_name))),
        password_hash: ActiveValue::Set(Some(hash_blocking(password).await?)),
        display_name: ActiveValue::Set(display_name.map(str::to_string)),
        ..Default::default()
    };
    active.insert(conn).await.map_err(|err| {
        YorishiroError::conflict_on_unique(
            err,
            format!("a user named '{user_name}' already exists in this tenant"),
        )
    })
}

/// Replaces a tenant-bound user's password and revokes every key issued to them.
///
/// # Errors
/// Rejects a weak password and users that are not bound to `tenant_id`.
pub(crate) async fn reset_tenant_password(
    conn: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
    user_id: uuid::Uuid,
    password: &str,
) -> Result<(), YorishiroError> {
    validate_password(password)?;
    let user = Entity::find_by_id(user_id)
        .filter(Column::TenantId.eq(tenant_id))
        .one(conn)
        .await
        .internal()?
        .or_not_found("user was not found")?;
    let mut active: ActiveModel = user.into();
    active.password_hash = ActiveValue::Set(Some(hash_blocking(password).await?));
    active.update(conn).await.internal()?;
    crate::models::access_keys::Entity::revoke_matching(
        conn,
        Condition::all()
            .add(crate::models::_entities::access_keys::Column::TenantId.eq(tenant_id))
            .add(crate::models::_entities::access_keys::Column::UserId.eq(user_id)),
    )
    .await
}
