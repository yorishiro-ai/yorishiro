//! Invite issuance and redemption.

use chrono::{DateTime, Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    QueryFilter,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_invites::Column;
use crate::models::api_keys::hash_key;
use crate::models::tenant_memberships::MembershipRole;

use super::{ActiveModel, Entity, Model, RedeemedInvite};

const INVITE_TOKEN_LENGTH: usize = 48;

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
    create_invite_at(conn, tenant_id, email, role, ttl, crate::time::now()).await
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
    let token = loco_rs::hash::random_string(INVITE_TOKEN_LENGTH);
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

/// Registers an invited user and consumes the invite only if registration and membership both succeed.
pub(crate) async fn register_user(
    db: &DatabaseConnection,
    raw_token: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<(crate::models::user_users::Model, RedeemedInvite), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    let invite = redeem_invite_at(&txn, raw_token, crate::time::now())
        .await?
        .ok_or_else(|| YorishiroError::ValidationFailed {
            message: "invite token is invalid, expired, or already used".into(),
            details: vec![],
            hint: "ask a tenant admin for a fresh invite".into(),
        })?;
    let user =
        crate::models::user_users::create_user(&txn, &invite.email, password, display_name).await?;
    crate::models::tenant_memberships::add_member(&txn, invite.tenant_id, user.id, invite.role)
        .await?;
    txn.commit().await.internal()?;
    Ok((user, invite))
}

/// Atomically consumes an unused, unexpired invite at the supplied time.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn redeem_invite_at(
    conn: &impl ConnectionTrait,
    raw_token: &str,
    now: DateTime<Utc>,
) -> Result<Option<RedeemedInvite>, YorishiroError> {
    let token_hash = hash_key(raw_token);

    let invite = Entity::update_many()
        .col_expr(Column::UsedAt, sea_orm::sea_query::Expr::value(now))
        .filter(Column::TokenHash.eq(token_hash))
        .filter(Column::UsedAt.is_null())
        .filter(Column::ExpiresAt.gt(now))
        .exec_with_returning(conn)
        .await
        .internal()?
        .into_iter()
        .next();

    let Some(invite) = invite else {
        return Ok(None);
    };

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
