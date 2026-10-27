//! Invite issuance and redemption.

use chrono::{DateTime, Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection,
    DatabaseTransaction, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_invites::Column;
use crate::models::api_keys::hash_key;
use crate::models::tenant_memberships::MembershipRole;

use super::{ActiveModel, Entity, Model, RedeemedInvite};

const INVITE_TOKEN_LENGTH: usize = 48;

async fn invite_tenant(txn: &DatabaseTransaction, raw_token: &str) -> Result<Uuid, YorishiroError> {
    Entity::find()
        .filter(Column::TokenHash.eq(hash_key(raw_token)))
        .one(txn)
        .await
        .internal()?
        .map(|invite| invite.tenant_id)
        .ok_or_else(|| YorishiroError::ValidationFailed {
            message: "invite token is invalid, expired, or already used".into(),
            details: vec![],
            hint: "ask a tenant admin for a fresh invite".into(),
        })
}

/// Accepts an invitation with proof of ownership of the existing account.
pub(crate) async fn accept_existing_user(
    db: &DatabaseConnection,
    raw_token: &str,
    email: &str,
    password: &str,
) -> Result<(crate::models::user_users::Model, RedeemedInvite), YorishiroError> {
    let user = crate::models::user_users::verify_login(db, email, password)
        .await?
        .ok_or(YorishiroError::Unauthenticated)?;
    let txn = crate::db::begin_write(db).await.internal()?;
    let invite = accept_in_transaction(&txn, raw_token, &user).await?;
    txn.commit().await.internal()?;
    Ok((user, invite))
}

/// Accepts an invite using an existing attributed credential, including passwordless accounts.
pub(crate) async fn accept_authenticated_user(
    db: &DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    raw_token: &str,
) -> Result<(crate::models::user_users::Model, RedeemedInvite), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    let target_tenant = invite_tenant(&txn, raw_token).await?;
    // Stable ordering prevents cross-tenant invite acceptances from deadlocking.
    let mut tenants = [actor.tenant_id, target_tenant];
    tenants.sort_unstable();
    for tenant_id in tenants {
        crate::db::lock_for_update(&txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
    }
    let current = crate::models::api_keys::Model::current_context(&txn, actor).await?;
    let user_id = current.user_id.ok_or(YorishiroError::Unauthenticated)?;
    let user = crate::models::user_users::Entity::find_by_id(user_id)
        .one(&txn)
        .await
        .internal()?
        .ok_or(YorishiroError::Unauthenticated)?;
    let invite = accept_in_transaction(&txn, raw_token, &user).await?;
    txn.commit().await.internal()?;
    Ok((user, invite))
}

async fn accept_in_transaction(
    txn: &DatabaseTransaction,
    raw_token: &str,
    user: &crate::models::user_users::Model,
) -> Result<RedeemedInvite, YorishiroError> {
    let tenant_id = invite_tenant(txn, raw_token).await?;
    crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
        .await
        .internal()?;
    let invite = redeem_invite_at(txn, raw_token, crate::time::now())
        .await?
        .ok_or_else(|| YorishiroError::ValidationFailed {
            message: "invite token is invalid, expired, or already used".into(),
            details: vec![],
            hint: "ask a tenant admin for a fresh invite".into(),
        })?;
    if invite.email != user.email {
        return Err(YorishiroError::Unauthenticated);
    }
    if crate::models::tenant_memberships::get_membership_role(txn, invite.tenant_id, user.id)
        .await?
        .is_some()
    {
        return Err(YorishiroError::Conflict {
            message: "this account already belongs to the invited tenant".into(),
        });
    }
    crate::models::tenant_memberships::add_member_in_transaction(
        txn,
        invite.tenant_id,
        user.id,
        invite.role,
    )
    .await?;
    Ok(invite)
}

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
    let tenant_id = invite_tenant(&txn, raw_token).await?;
    crate::db::lock_for_update(&txn, &format!("tenant-membership:{tenant_id}"))
        .await
        .internal()?;
    let invite = redeem_invite_at(&txn, raw_token, crate::time::now())
        .await?
        .ok_or_else(|| YorishiroError::ValidationFailed {
            message: "invite token is invalid, expired, or already used".into(),
            details: vec![],
            hint: "ask a tenant admin for a fresh invite".into(),
        })?;
    let user = crate::models::tenancy::register_member(
        &txn,
        invite.tenant_id,
        &invite.email,
        password,
        display_name,
        invite.role,
    )
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
