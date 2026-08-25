//! Reading and writing the OAuth identity columns on `identity_users`.
//!
//! The query alone: what to do with a lookup's result (first login vs. returning user, tenant and workspace auto-provisioning) is `services::oauth::users`'s.

use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, SqlErr,
};
use uuid::Uuid;
use yorishiro_core::error::{ResultExt, YorishiroError};
use yorishiro_core::models::_entities::identity_users;

pub struct OAuthUser {
    pub id: Uuid,
    pub email: String,
}

/// Looks up a user previously provisioned through this exact provider + subject id pair.
/// Keyed on the pair (not email alone) because the subject id is what the provider actually guarantees stable and unique: an email can be reassigned or changed at the provider, but `sub` never changes for the same account.
/// Returns `None` on first login for a given identity, whether or not a different (e.g. password-based) account already exists under the same email.
/// Callers decide how to reconcile that (see `services::oauth::users::find_or_create`).
pub async fn find_by_oauth_identity(
    conn: &impl ConnectionTrait,
    provider: &str,
    subject_id: &str,
) -> Result<Option<OAuthUser>, YorishiroError> {
    let user = identity_users::Entity::find()
        .filter(identity_users::Column::OauthProvider.eq(provider))
        .filter(identity_users::Column::OauthSubjectId.eq(subject_id))
        .one(conn)
        .await
        .internal()?;

    Ok(user.map(|u| OAuthUser {
        id: u.id,
        email: u.email,
    }))
}

#[derive(Debug)]
pub enum CreateOauthUserError {
    /// Some unique constraint on `identity_users` rejected the insert.
    /// This can only be the `email` column's own constraint (a genuinely different account already holds this email): `find_or_create` holds `pg_advisory_xact_lock` for this exact `(provider, subject_id)` for the whole first-login path, including a re-check via `find_by_oauth_identity` immediately before this insert, so no other caller can be concurrently inserting the same identity.
    /// `users_oauth_identity_idx` cannot be the constraint that fired here.
    UniqueViolation,
    Other(YorishiroError),
}

/// Creates a new OAuth-provisioned user row (`password_hash` left `NULL`, per `users_auth_method_check`).
/// Does not touch tenancy: see `services::oauth::users::find_or_create` for the caller that wires a freshly created user into a tenant, workspace and membership.
///
/// Takes `&impl ConnectionTrait` (rather than a pool handle) so `find_or_create` can run this on the same transaction as `tenancy::add_member`: both must succeed or fail together, or a crash between them would leave an orphaned user row with no tenant membership, and every later login for that identity would then resolve to a permanent `ScopeInsufficient`.
pub async fn create_oauth_user(
    conn: &impl ConnectionTrait,
    email: &str,
    display_name: Option<&str>,
    provider: &str,
    subject_id: &str,
) -> Result<OAuthUser, CreateOauthUserError> {
    let active = identity_users::ActiveModel {
        email: ActiveValue::Set(email.to_string()),
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
    Ok(OAuthUser {
        id: user.id,
        email: user.email,
    })
}
