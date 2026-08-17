use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use chrono::{DateTime, Utc};
use sea_query::{Alias, Expr, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::tenancy::UserRecord;

#[derive(Iden)]
pub(super) enum Users {
    Table,
    Id,
    Email,
    PasswordHash,
    DisplayName,
    CreatedAt,
}

fn hash_password(password: &str) -> Result<String, YorishiroError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| YorishiroError::Internal(anyhow::anyhow!("failed to hash password: {err}")))
}

fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Creates a human user account.
/// Passwords are hashed with argon2 (the current OWASP recommendation for password storage) before ever reaching the database.
///
/// Takes `&mut PgConnection` (rather than `&PgPool`) so a caller can compose this with `add_member` (and anything else) in one transaction: the two must succeed or fail together, or a failure between them leaves an orphaned user row that can never join a tenant (a real bug this signature exists to make impossible: see `signup`, which now wraps both calls in one transaction on `identity_pool`, the only pool with insert privileges on `identity.users`/`identity.tenant_memberships` (`yorishiro_app`, the tenant-scoped role, has none)).
/// Pass `&mut pool.acquire().await?` for a standalone call.
pub async fn create_user(
    conn: &mut PgConnection,
    email: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<UserRecord, YorishiroError> {
    let password_hash = hash_password(password)?;
    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Users::Table))
        .columns([Users::Email, Users::PasswordHash, Users::DisplayName])
        .values_panic([email.into(), password_hash.into(), display_name.into()])
        .returning(Query::returning().columns([
            Users::Id,
            Users::Email,
            Users::DisplayName,
            Users::CreatedAt,
        ]))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, UserRecord, _>(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .map_err(|err| {
            if let sqlx::Error::Database(db_err) = &err
                && db_err.is_unique_violation()
            {
                YorishiroError::Conflict {
                    message: format!("a user with email '{email}' already exists"),
                }
            } else {
                YorishiroError::Internal(err.into())
            }
        })
}

/// Looks up an existing user by email, without touching their password hash.
/// Used by member management (adding an *existing* account to another tenant) to resolve an email to a `user_id` before calling `add_member`, as opposed to signup, which creates the account.
///
/// Takes `&mut PgConnection` (rather than `&PgPool`) so a find-or-create caller (e.g. an OAuth login/callback flow: look up by email, `create_user` if absent, then `add_member`) can run the lookup, insert, and membership grant in one transaction.
/// This does *not* prevent two concurrent callbacks for the same new email from both seeing "no user" and both attempting `create_user`: one still loses to `create_user`'s unique-email constraint and its whole transaction rolls back.
/// What the shared transaction buys is that the loser's flow fails cleanly (a `Conflict`, safe to retry as a plain lookup) instead of leaving a user row with no tenant membership, the way two independent pool round-trips would.
/// Pass `&mut pool.acquire().await?` for a standalone call.
pub async fn get_user_by_email(
    conn: &mut PgConnection,
    email: &str,
) -> Result<Option<UserRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns([
            Users::Id,
            Users::Email,
            Users::DisplayName,
            Users::CreatedAt,
        ])
        .from((Alias::new("identity"), Users::Table))
        .and_where(Expr::col(Users::Email).eq(email))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, UserRecord, _>(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .internal()
}

/// Verifies an email/password pair against the stored argon2 hash, returning the matching user on success.
/// Backs the `/auth/login` REST endpoint.
pub async fn verify_login(
    pool: &PgPool,
    email: &str,
    password: &str,
) -> Result<Option<UserRecord>, YorishiroError> {
    #[derive(sqlx::FromRow)]
    struct UserWithHash {
        id: Uuid,
        email: String,
        display_name: Option<String>,
        created_at: DateTime<Utc>,
        password_hash: String,
    }

    let (sql, values) = Query::select()
        .columns([
            Users::Id,
            Users::Email,
            Users::DisplayName,
            Users::CreatedAt,
            Users::PasswordHash,
        ])
        .from((Alias::new("identity"), Users::Table))
        .and_where(Expr::col(Users::Email).eq(email))
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<UserWithHash> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?;

    let Some(row) = row else {
        return Ok(None);
    };

    if verify_password(password, &row.password_hash) {
        Ok(Some(UserRecord {
            id: row.id,
            email: row.email,
            display_name: row.display_name,
            created_at: row.created_at,
        }))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
#[path = "../../../tests/repositories/tenancy/users.rs"]
mod tests;
