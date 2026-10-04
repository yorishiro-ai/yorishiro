//! Invite issuance and redemption.

use chrono::{DateTime, Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_invites::Column;
use crate::models::api_keys::{hash_key, random_hex};
use crate::models::tenant_memberships::MembershipRole;

use super::{ActiveModel, Entity, Model, RedeemedInvite};

const INVITE_TOKEN_BYTES: usize = 24;

/// Creates an invite token for `email` to join `tenant_id` with `role`.
/// Returns the record alongside the plaintext token: like API keys, only its SHA-256 hash is persisted, so this is the only place the plaintext is ever available.
/// Callers must surface it themselves (printed by the admin CLI today; a transactional-email integration is not provided).
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn create_invite(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    email: &str,
    role: MembershipRole,
    ttl: Duration,
) -> Result<(Model, String), YorishiroError> {
    create_invite_at(conn, tenant_id, email, role, ttl, Utc::now()).await
}

/// [`create_invite`] with the issue time supplied by the caller, so expiry boundaries can be tested without a clock.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn create_invite_at(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    email: &str,
    role: MembershipRole,
    ttl: Duration,
    now: DateTime<Utc>,
) -> Result<(Model, String), YorishiroError> {
    let token = random_hex(INVITE_TOKEN_BYTES);
    let token_hash = hash_key(&token);
    let expires_at = now + ttl;

    let active = ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        email: ActiveValue::Set(email.to_string()),
        role: ActiveValue::Set(role.as_db_str().to_string()),
        token_hash: ActiveValue::Set(token_hash),
        expires_at: ActiveValue::Set(expires_at.into()),
        ..Default::default()
    };

    let invite = active.insert(conn).await.internal()?;
    Ok((invite, token))
}

/// Redeems an invite token: atomically marks it used and returns the tenant/email/role it grants, or `None` if the token doesn't match any invite, is already used, or has expired.
///
/// The lookup and the `used_at` update happen in a single statement (`UpdateMany` with all three conditions in its `WHERE`), so two concurrent redemptions of the same token can't both succeed: whichever commits first's `used_at IS NULL` no longer holds for the loser.
pub(crate) async fn redeem_invite(
    conn: &impl ConnectionTrait,
    raw_token: &str,
) -> Result<Option<RedeemedInvite>, YorishiroError> {
    redeem_invite_at(conn, raw_token, Utc::now()).await
}

/// [`redeem_invite`] with the redemption time supplied by the caller, so expiry boundaries can be tested without a clock.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn redeem_invite_at(
    conn: &impl ConnectionTrait,
    raw_token: &str,
    now: DateTime<Utc>,
) -> Result<Option<RedeemedInvite>, YorishiroError> {
    let token_hash = hash_key(raw_token);

    // Read first to build the response: the update itself does not return rows affected as model data, and a second SELECT after the UPDATE could observe a different row (e.g. one this same call just marked used) if invites were ever deletable, which they are not, so this is safe, not merely convenient.
    let invite = Entity::find()
        .filter(Column::TokenHash.eq(token_hash.clone()))
        .filter(Column::UsedAt.is_null())
        .filter(Column::ExpiresAt.gt(now))
        .one(conn)
        .await
        .internal()?;

    let Some(invite) = invite else {
        return Ok(None);
    };

    let update_result = Entity::update_many()
        .col_expr(Column::UsedAt, sea_orm::sea_query::Expr::value(now))
        .filter(Column::Id.eq(invite.id))
        .filter(Column::UsedAt.is_null())
        .filter(Column::ExpiresAt.gt(now))
        .exec(conn)
        .await
        .internal()?;

    if update_result.rows_affected == 0 {
        // Lost the race: another concurrent redemption already claimed this token between the read above and this UPDATE.
        return Ok(None);
    }

    let role = MembershipRole::from_db_str(&invite.role).ok_or_else(|| {
        YorishiroError::Internal(anyhow::anyhow!(
            "unknown membership role in database: {}",
            invite.role
        ))
    })?;

    Ok(Some(RedeemedInvite {
        tenant_id: invite.tenant_id,
        email: invite.email,
        role,
    }))
}
