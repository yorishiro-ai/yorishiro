//! User creation, self-service profiles and authentication lookups.

use loco_rs::hash;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, SqlErr,
};
use sea_orm::{Condition, PaginatorTrait, QueryOrder, QuerySelect};

use super::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::user_users::Column;

pub(crate) async fn list_by_operator(
    conn: &impl ConnectionTrait,
    limit: u64,
    offset: u64,
) -> Result<Vec<Model>, YorishiroError> {
    Entity::find()
        .order_by_asc(Column::Id)
        .limit(limit.clamp(1, 100))
        .offset(offset)
        .all(conn)
        .await
        .internal()
}

/// Deletes an account with a trusted operator identity.
/// A root user who is the only owner of a tenant takes that disabled tenant, and the users bound to it, along with them.
///
/// # Errors
/// Rejects a mismatched confirmation, an enabled owned tenant, and any other tenant membership.
pub async fn delete_by_operator(
    db: &sea_orm::DatabaseConnection,
    user_id: uuid::Uuid,
    confirmed_user_id: uuid::Uuid,
) -> Result<(), YorishiroError> {
    if user_id != confirmed_user_id {
        return Err(YorishiroError::Conflict {
            message: "confirmed_user_id must match user_id".into(),
        });
    }
    let txn = crate::db::begin_write(db).await.internal()?;
    // The row lock also serializes concurrent membership/key inserts through their user foreign key.
    Entity::find_by_id(user_id)
        .lock_exclusive()
        .one(&txn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found("user not found"))?;
    use crate::models::_entities::tenant_memberships as memberships;
    // A root user who is the only owner of a tenant takes that tenant with them; any other membership must be removed first.
    let held = memberships::Entity::find()
        .filter(memberships::Column::UserId.eq(user_id))
        .all(&txn)
        .await
        .internal()?;
    let mut owned_tenants = Vec::new();
    for membership in held {
        let owners = memberships::Entity::find()
            .filter(memberships::Column::TenantId.eq(membership.tenant_id))
            .filter(
                memberships::Column::Role
                    .eq(crate::models::tenant_memberships::MembershipRole::Owner.as_db_str()),
            )
            .count(&txn)
            .await
            .internal()?;
        if membership.role == crate::models::tenant_memberships::MembershipRole::Owner.as_db_str()
            && owners == 1
        {
            owned_tenants.push(membership.tenant_id);
        } else {
            return Err(YorishiroError::Conflict {
                message: format!(
                    "remove the user's membership in tenant {} before deleting the account",
                    membership.tenant_id
                ),
            });
        }
    }
    for tenant_id in owned_tenants {
        crate::models::tenant_tenants::retirement::delete_owned_by_user(&txn, tenant_id).await?;
    }
    // Revoke before ON DELETE SET NULL can detach user attribution from authenticating keys.
    crate::models::api_keys::Entity::revoke_matching(
        &txn,
        Condition::all().add(crate::models::_entities::api_keys::Column::UserId.eq(user_id)),
    )
    .await?;
    Entity::delete_by_id(user_id).exec(&txn).await.internal()?;
    txn.commit().await.internal()?;
    Ok(())
}

pub(crate) async fn current_profile(
    conn: &impl ConnectionTrait,
    actor: &crate::models::api_keys::AuthContext,
) -> Result<Model, YorishiroError> {
    let current = crate::models::api_keys::Model::current_context(conn, actor).await?;
    let user_id = current
        .user_id
        .ok_or_else(|| YorishiroError::ScopeInsufficient {
            message: "service credentials have no user profile".into(),
            hint: "use a user-attributed credential".into(),
        })?;
    Entity::find_by_id(user_id)
        .one(conn)
        .await
        .internal()?
        .ok_or(YorishiroError::Unauthenticated)
}

pub(crate) async fn update_profile(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    display_name: Option<String>,
) -> Result<Model, YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::db::lock_for_update(&txn, &format!("tenant-membership:{}", actor.tenant_id))
        .await
        .internal()?;
    let user = current_profile(&txn, actor).await?;
    let display_name = display_name.map(|name| name.trim().to_owned());
    if display_name
        .as_ref()
        .is_some_and(|name| name.is_empty() || name.chars().count() > 100)
    {
        return Err(YorishiroError::Conflict {
            message: "display name must contain 1 to 100 characters, or be null".into(),
        });
    }
    let mut active: ActiveModel = user.into();
    active.display_name = ActiveValue::Set(display_name);
    let user = active.update(&txn).await.internal()?;
    txn.commit().await.internal()?;
    Ok(user)
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
        if matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
            YorishiroError::Conflict {
                message: format!("a user with email '{email}' already exists"),
            }
        } else {
            YorishiroError::Internal(err.into())
        }
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
        .filter(Column::UserName.eq(user_name))
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
const MAX_USER_NAME_CHARS: usize = 64;

fn validate_tenant_credentials(user_name: &str, password: &str) -> Result<(), YorishiroError> {
    if user_name.is_empty()
        || user_name.chars().count() > MAX_USER_NAME_CHARS
        || !user_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(YorishiroError::Conflict {
            message: format!(
                "user_name must contain 1 to {MAX_USER_NAME_CHARS} letters, digits, '.', '_' or '-'"
            ),
        });
    }
    if password.chars().count() < MIN_TENANT_PASSWORD_CHARS {
        return Err(YorishiroError::Conflict {
            message: format!(
                "password must contain at least {MIN_TENANT_PASSWORD_CHARS} characters"
            ),
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
        password_hash: ActiveValue::Set(Some(hash_blocking(password).await?)),
        display_name: ActiveValue::Set(display_name.map(str::to_string)),
        ..Default::default()
    };
    active.insert(conn).await.map_err(|err| {
        if matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
            YorishiroError::Conflict {
                message: format!("a user named '{user_name}' already exists in this tenant"),
            }
        } else {
            YorishiroError::Internal(err.into())
        }
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
    if password.chars().count() < MIN_TENANT_PASSWORD_CHARS {
        return Err(YorishiroError::Conflict {
            message: format!(
                "password must contain at least {MIN_TENANT_PASSWORD_CHARS} characters"
            ),
        });
    }
    let user = Entity::find_by_id(user_id)
        .filter(Column::TenantId.eq(tenant_id))
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found("tenant user not found"))?;
    let mut active: ActiveModel = user.into();
    active.password_hash = ActiveValue::Set(Some(hash_blocking(password).await?));
    active.update(conn).await.internal()?;
    crate::models::api_keys::Entity::revoke_matching(
        conn,
        Condition::all()
            .add(crate::models::_entities::api_keys::Column::TenantId.eq(tenant_id))
            .add(crate::models::_entities::api_keys::Column::UserId.eq(user_id)),
    )
    .await
}

/// Looks up a user by email, for `POST /api/members` (which attaches an *existing* account by email, never creates one).
pub(crate) async fn get_user_by_email(
    conn: &impl ConnectionTrait,
    email: &str,
) -> Result<Option<Model>, YorishiroError> {
    Entity::find()
        .filter(Column::Email.eq(email))
        .one(conn)
        .await
        .internal()
}
