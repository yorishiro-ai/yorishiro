//! User creation and authentication lookups.

use loco_rs::hash;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, SqlErr,
};

use super::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::user_users::Column;

/// Creates a human user account.
/// The password is hashed with `loco_rs::hash` (Argon2id) before ever reaching the database.
///
/// Takes `&impl ConnectionTrait` rather than a pool handle so a caller can compose this with `add_member` in one transaction: the two must succeed or fail together, or a failure between them leaves an orphaned user row that can never join a tenant (see `signup`, which wraps both in one transaction).
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn create_user(
    conn: &impl ConnectionTrait,
    email: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<Model, YorishiroError> {
    let password_hash = hash::hash_password(password).internal()?;

    let active = ActiveModel {
        email: ActiveValue::Set(email.to_string()),
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

/// Verifies an email/password pair against the stored Argon2id hash, returning the matching user on success.
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

    let Some(user) = user else {
        return Ok(None);
    };

    let matches = user
        .password_hash
        .as_deref()
        .is_some_and(|hash| hash::verify_password(password, hash));

    Ok(matches.then_some(user))
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
