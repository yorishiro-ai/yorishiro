use chrono::{DateTime, Duration, Utc};
use sea_query::{Alias, Expr, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use uuid::Uuid;

use super::get_tenant;
use crate::error::{ResultExt, YorishiroError};
use crate::models::tenancy::{InviteRecord, MembershipRole};
use crate::services::auth::{hash_key, random_hex};

#[derive(Iden)]
enum Invites {
    Table,
    Id,
    TenantId,
    Email,
    Role,
    TokenHash,
    ExpiresAt,
    UsedAt,
    CreatedAt,
}

const INVITE_TOKEN_BYTES: usize = 24;

#[derive(sqlx::FromRow)]
struct InviteRow {
    id: Uuid,
    tenant_id: Uuid,
    email: String,
    role: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

fn invite_columns() -> [Invites; 6] {
    [
        Invites::Id,
        Invites::TenantId,
        Invites::Email,
        Invites::Role,
        Invites::ExpiresAt,
        Invites::CreatedAt,
    ]
}

impl InviteRow {
    fn into_record(self) -> Result<InviteRecord, YorishiroError> {
        let role = MembershipRole::from_db_str(&self.role).ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!(
                "unknown membership role in database: {}",
                self.role
            ))
        })?;
        Ok(InviteRecord {
            id: self.id,
            tenant_id: self.tenant_id,
            email: self.email,
            role,
            expires_at: self.expires_at,
            created_at: self.created_at,
        })
    }
}

/// Creates an invite token for `email` to join `tenant_id` with `role`.
/// Returns the record alongside the plaintext token: like API keys, only its SHA-256 hash is persisted (a KDF like argon2 isn't needed here either, for the same reason: the token already carries enough entropy that offline brute-forcing isn't realistic), so this is the only place the plaintext is ever available.
/// Callers must surface it themselves (e.g. print it, or send it by email once a transactional-email integration exists).
pub async fn create_invite(
    pool: &PgPool,
    tenant_id: Uuid,
    email: &str,
    role: MembershipRole,
    ttl: Duration,
) -> Result<(InviteRecord, String), YorishiroError> {
    let mut conn = pool.acquire().await.internal()?;
    get_tenant(&mut *conn, tenant_id).await?;

    let token = random_hex(INVITE_TOKEN_BYTES);
    let token_hash = hash_key(&token);
    let expires_at = Utc::now() + ttl;

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Invites::Table))
        .columns([
            Invites::TenantId,
            Invites::Email,
            Invites::Role,
            Invites::TokenHash,
            Invites::ExpiresAt,
        ])
        .values_panic([
            tenant_id.into(),
            email.into(),
            role.as_db_str().into(),
            token_hash.into(),
            expires_at.into(),
        ])
        .returning(Query::returning().columns(invite_columns()))
        .build_sqlx(PostgresQueryBuilder);

    let row: InviteRow = sqlx::query_as_with(&sql, values)
        .fetch_one(pool)
        .await
        .internal()?;

    Ok((row.into_record()?, token))
}

/// Redeems an invite token: atomically marks it used and returns the tenant/email/role it grants, or `None` if the token doesn't match any invite, is already used, or has expired.
/// The lookup and the `used_at` update happen in a single statement so two concurrent redemptions of the same token can't both succeed.
pub async fn redeem_invite(
    pool: &PgPool,
    raw_token: &str,
) -> Result<Option<InviteRecord>, YorishiroError> {
    let token_hash = hash_key(raw_token);

    let (sql, values) = Query::update()
        .table((Alias::new("identity"), Invites::Table))
        .values([(Invites::UsedAt, Expr::current_timestamp().into())])
        .and_where(Expr::col(Invites::TokenHash).eq(token_hash))
        .and_where(Expr::col(Invites::UsedAt).is_null())
        .and_where(Expr::col(Invites::ExpiresAt).gt(Expr::current_timestamp()))
        .returning(Query::returning().columns(invite_columns()))
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<InviteRow> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?;

    row.map(InviteRow::into_record).transpose()
}

#[cfg(test)]
#[path = "../../../tests/models/tenancy/invites.rs"]
mod tests;
