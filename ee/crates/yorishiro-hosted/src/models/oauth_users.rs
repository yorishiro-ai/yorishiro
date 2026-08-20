//! Reading and writing the OAuth identity columns on `identity.users`.
//!
//! `identity.users` is base's table; `oauth_provider`/`oauth_subject_id` are columns this repo's own migration adds (see `migrations/20260730100001_oauth_identity.sql`).
//! `yorishiro-core`'s `UserRecord`/`users` module knows nothing about them, and this crate isn't allowed to modify that upstream crate, so the two columns' queries live here instead.
//!
//! The query alone: what to do with a lookup's result (first login vs. returning user, tenant/workspace auto-provisioning) is `services::oauth::users`'s.

use sea_query::{Alias, Expr, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;
use yorishiro_core::{ResultExt, YorishiroError};

#[derive(Iden)]
enum Users {
    Table,
    Id,
    Email,
    DisplayName,
    OauthProvider,
    OauthSubjectId,
}

pub struct OAuthUser {
    pub id: Uuid,
    pub email: String,
}

/// Looks up a user previously provisioned through this exact provider + subject id pair.
/// Keyed on the pair (not email alone) because the subject id is what the provider actually guarantees stable and unique: an email can be reassigned or changed at the provider, but `sub` never changes for the same account.
/// Returns `None` on first login for a given identity, whether or not a *different* (e.g. password-based) account already exists under the same email.
/// Callers decide how to reconcile that (see `services::oauth::users::find_or_create`).
pub async fn find_by_oauth_identity(
    pool: &PgPool,
    provider: &str,
    subject_id: &str,
) -> Result<Option<OAuthUser>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns([Users::Id, Users::Email])
        .from((Alias::new("identity"), Users::Table))
        .and_where(Expr::col(Users::OauthProvider).eq(provider))
        .and_where(Expr::col(Users::OauthSubjectId).eq(subject_id))
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<(Uuid, String)> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?;

    Ok(row.map(|(id, email)| OAuthUser { id, email }))
}

#[derive(Debug)]
pub enum CreateOauthUserError {
    /// Some unique constraint on `identity.users` rejected the insert.
    /// This can only be the `email` column's own constraint (a genuinely different account already holds this email):
    /// `find_or_create` holds `pg_advisory_xact_lock` for this exact `(provider, subject_id)` for the whole first-login path, including a re-check via `find_by_oauth_identity` immediately before this insert, so no other caller can be concurrently inserting the same identity.
    /// `users_oauth_identity_idx` cannot be the constraint that fired here.
    UniqueViolation,
    Other(YorishiroError),
}

/// Creates a new OAuth-provisioned user row (`password_hash` left `NULL`, per the migration's CHECK constraint).
/// Does not touch tenancy: see `services::oauth::users::find_or_create` for the caller that wires a freshly created user into a tenant/workspace/membership.
///
/// Takes `&mut PgConnection` (rather than `&PgPool`) so `find_or_create` can run this on the same transaction as `tenancy::add_member`, mirroring how base's `yorishiro_core::models::tenancy::create_user`/`add_member` compose (see their doc comments): both must run in one transaction, or a crash between this insert and `add_member` would leave an orphaned user row with no tenant membership, and every later login for that identity would then resolve to a permanent `ScopeInsufficient`.
pub async fn create_oauth_user(
    conn: &mut PgConnection,
    email: &str,
    display_name: Option<&str>,
    provider: &str,
    subject_id: &str,
) -> Result<OAuthUser, CreateOauthUserError> {
    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Users::Table))
        .columns([
            Users::Email,
            Users::DisplayName,
            Users::OauthProvider,
            Users::OauthSubjectId,
        ])
        .values_panic([
            email.into(),
            display_name.into(),
            provider.into(),
            subject_id.into(),
        ])
        .returning(Query::returning().columns([Users::Id, Users::Email]))
        .build_sqlx(PostgresQueryBuilder);

    let result: Result<(Uuid, String), sqlx::Error> = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await;

    if let Err(sqlx::Error::Database(db_err)) = &result
        && db_err.is_unique_violation()
    {
        return Err(CreateOauthUserError::UniqueViolation);
    }
    let (id, email) = result.internal().map_err(CreateOauthUserError::Other)?;

    Ok(OAuthUser { id, email })
}

#[cfg(test)]
#[path = "../../tests/models/oauth_users.rs"]
mod tests;
