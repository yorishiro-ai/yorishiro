//! Reading and writing the OAuth identity columns on `user_users`.
//!
//! OAuth identity persistence and first-login tenant/workspace provisioning.

mod provisioning;

pub use provisioning::find_or_create;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::user_users;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, SqlErr,
};
use uuid::Uuid;

pub(crate) struct OAuthUser {
    pub(crate) id: Uuid,
}

/// Looks up a user previously provisioned through this exact provider and subject id pair.
/// The pair is the key, not the email, because a provider can reassign an email but never changes `sub`.
/// Returns `None` on the first login of an identity, even when another account already holds the same email.
pub(crate) async fn find_by_oauth_identity(
    conn: &impl ConnectionTrait,
    provider: &str,
    subject_id: &str,
) -> Result<Option<OAuthUser>, YorishiroError> {
    let user = user_users::Entity::find()
        .filter(user_users::Column::TenantId.is_null())
        .filter(user_users::Column::OauthProvider.eq(provider))
        .filter(user_users::Column::OauthSubjectId.eq(subject_id))
        .one(conn)
        .await
        .internal()?;

    Ok(user.map(|u| OAuthUser { id: u.id }))
}

pub(crate) enum CreateOauthUserError {
    /// Some unique constraint on `user_users` rejected the insert.
    /// This can only be the `email` constraint: a different account already holds the email.
    /// The first-login path holds an advisory lock on the identity and re-checks it before the insert, so the identity index cannot be the one that fired.
    UniqueViolation,
    Other(YorishiroError),
}

/// Creates a new OAuth-provisioned user row (`password_hash` left `NULL`, per `users_auth_method_check`).
/// Does not touch tenancy: the caller wires the new user into a tenant and membership.
///
/// Takes `&impl ConnectionTrait` so user creation and membership creation can run in one transaction.
/// Without that transaction, a crash can leave a user with no membership and prevent later sign-ins.
pub(crate) async fn create_oauth_user(
    conn: &impl ConnectionTrait,
    email: &str,
    display_name: Option<&str>,
    provider: &str,
    subject_id: &str,
) -> Result<OAuthUser, CreateOauthUserError> {
    let active = user_users::ActiveModel {
        email: ActiveValue::Set(Some(email.to_string())),
        display_name: ActiveValue::Set(display_name.map(str::to_string)),
        oauth_provider: ActiveValue::Set(Some(provider.to_string())),
        oauth_subject_id: ActiveValue::Set(Some(subject_id.to_string())),
        ..Default::default()
    };

    let result = active.insert(conn).await;

    if let Err(err) = &result
        && matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_)))
    {
        return Err(CreateOauthUserError::UniqueViolation);
    }

    let user = result.internal().map_err(CreateOauthUserError::Other)?;
    Ok(OAuthUser { id: user.id })
}
