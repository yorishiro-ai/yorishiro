//! Control-plane CRUD for users, invites, and tenant memberships: signup, login, and the
//! `admin create-invite` chain.
//!
//! Everything here runs on `ctx.db` (Loco's own migration-role connection), never the
//! RLS-scoped tenant pool: no workspace exists yet for RLS to scope by, the same reasoning
//! `TenantDb::connect`'s doc comment gives for the identity pool.

use chrono::{DateTime, Duration, Utc};
use loco_rs::hash;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, SqlErr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{identity_invites, identity_tenant_memberships, identity_users};
use crate::services::auth::{ApiKeyScope, hash_key, random_hex};

/// Mirrors the `identity_tenant_memberships.role` check constraint (`owner`/`admin`/`member`/`viewer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MembershipRole {
    Owner,
    Admin,
    Member,
    Viewer,
}

impl MembershipRole {
    pub fn as_db_str(self) -> &'static str {
        match self {
            MembershipRole::Owner => "owner",
            MembershipRole::Admin => "admin",
            MembershipRole::Member => "member",
            MembershipRole::Viewer => "viewer",
        }
    }

    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(MembershipRole::Owner),
            "admin" => Some(MembershipRole::Admin),
            "member" => Some(MembershipRole::Member),
            "viewer" => Some(MembershipRole::Viewer),
            _ => None,
        }
    }

    /// The highest API key scope a member with this role may be issued.
    pub fn max_scope(self) -> ApiKeyScope {
        match self {
            MembershipRole::Owner | MembershipRole::Admin => ApiKeyScope::Migration,
            MembershipRole::Member => ApiKeyScope::Write,
            MembershipRole::Viewer => ApiKeyScope::Read,
        }
    }
}

/// Creates a human user account. The password is hashed with `loco_rs::hash` (Argon2id) before
/// ever reaching the database.
///
/// Takes `&impl ConnectionTrait` rather than a pool handle so a caller can compose this with
/// `add_member` in one transaction: the two must succeed or fail together, or a failure between
/// them leaves an orphaned user row that can never join a tenant (see `signup`, which wraps both
/// in one transaction).
pub async fn create_user(
    conn: &impl ConnectionTrait,
    email: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<identity_users::Model, YorishiroError> {
    let password_hash =
        hash::hash_password(password).map_err(|err| YorishiroError::Internal(err.into()))?;

    let active = identity_users::ActiveModel {
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

/// Verifies an email/password pair against the stored Argon2id hash, returning the matching user
/// on success. An OAuth-only account (`password_hash = NULL`) never matches, same as a wrong
/// password: `loco_rs::hash::verify_password` needs a hash to compare against.
pub async fn verify_login(
    conn: &impl ConnectionTrait,
    email: &str,
    password: &str,
) -> Result<Option<identity_users::Model>, YorishiroError> {
    let user = identity_users::Entity::find()
        .filter(identity_users::Column::Email.eq(email))
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

/// Adds (or updates the role of) a user's membership in a tenant.
///
/// Takes `&impl ConnectionTrait` so a caller can compose this with `create_user` in one
/// transaction, same reasoning as `create_user`'s doc comment.
pub async fn add_member(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<(), YorishiroError> {
    use sea_orm::sea_query::OnConflict;

    let active = identity_tenant_memberships::ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(role.as_db_str().to_string()),
        ..Default::default()
    };

    identity_tenant_memberships::Entity::insert(active)
        .on_conflict(
            OnConflict::columns([
                identity_tenant_memberships::Column::TenantId,
                identity_tenant_memberships::Column::UserId,
            ])
            .update_column(identity_tenant_memberships::Column::Role)
            .to_owned(),
        )
        .exec(conn)
        .await
        .internal()?;

    Ok(())
}

/// Looks up a single user's role within a tenant, or `None` if they aren't a member.
pub async fn get_membership_role(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<MembershipRole>, YorishiroError> {
    let membership = identity_tenant_memberships::Entity::find()
        .filter(identity_tenant_memberships::Column::TenantId.eq(tenant_id))
        .filter(identity_tenant_memberships::Column::UserId.eq(user_id))
        .one(conn)
        .await
        .internal()?;

    Ok(membership.and_then(|m| MembershipRole::from_db_str(&m.role)))
}

const INVITE_TOKEN_BYTES: usize = 24;

/// Creates an invite token for `email` to join `tenant_id` with `role`.
/// Returns the record alongside the plaintext token: like API keys, only its SHA-256 hash is
/// persisted, so this is the only place the plaintext is ever available. Callers must surface it
/// themselves (printed by the admin CLI today; a transactional-email integration is not
/// provided).
pub async fn create_invite(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    email: &str,
    role: MembershipRole,
    ttl: Duration,
) -> Result<(identity_invites::Model, String), YorishiroError> {
    let token = random_hex(INVITE_TOKEN_BYTES);
    let token_hash = hash_key(&token);
    let expires_at = Utc::now() + ttl;

    let active = identity_invites::ActiveModel {
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

/// What a redeemed invite grants: resolved once, since the invite row is consumed by the same
/// call that reads it.
pub struct RedeemedInvite {
    pub tenant_id: Uuid,
    pub email: String,
    pub role: MembershipRole,
}

/// Redeems an invite token: atomically marks it used and returns the tenant/email/role it
/// grants, or `None` if the token doesn't match any invite, is already used, or has expired.
///
/// The lookup and the `used_at` update happen in a single statement (`UpdateMany` with all three
/// conditions in its `WHERE`), so two concurrent redemptions of the same token can't both
/// succeed: whichever commits first's `used_at IS NULL` no longer holds for the loser.
pub async fn redeem_invite(
    conn: &impl ConnectionTrait,
    raw_token: &str,
) -> Result<Option<RedeemedInvite>, YorishiroError> {
    let token_hash = hash_key(raw_token);
    let now = Utc::now();

    // Read first to build the response: the update itself does not return rows affected as
    // model data, and a second SELECT after the UPDATE could observe a different row (e.g. one
    // this same call just marked used) if invites were ever deletable, which they are not, so
    // this is safe, not merely convenient.
    let invite = identity_invites::Entity::find()
        .filter(identity_invites::Column::TokenHash.eq(token_hash.clone()))
        .filter(identity_invites::Column::UsedAt.is_null())
        .filter(identity_invites::Column::ExpiresAt.gt(now))
        .one(conn)
        .await
        .internal()?;

    let Some(invite) = invite else {
        return Ok(None);
    };

    let update_result = identity_invites::Entity::update_many()
        .col_expr(
            identity_invites::Column::UsedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .filter(identity_invites::Column::Id.eq(invite.id))
        .filter(identity_invites::Column::UsedAt.is_null())
        .filter(identity_invites::Column::ExpiresAt.gt(now))
        .exec(conn)
        .await
        .internal()?;

    if update_result.rows_affected == 0 {
        // Lost the race: another concurrent redemption already claimed this token between the
        // read above and this UPDATE.
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

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub name: String,
}

/// Every workspace under `tenant_id`, for the signup response (which workspaces the new member
/// can now log into).
pub async fn list_workspaces(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<Vec<WorkspaceSummary>, YorishiroError> {
    use crate::models::_entities::identity_workspaces;

    let workspaces = identity_workspaces::Entity::find()
        .filter(identity_workspaces::Column::TenantId.eq(tenant_id))
        .all(conn)
        .await
        .internal()?;

    Ok(workspaces
        .into_iter()
        .map(|w| WorkspaceSummary {
            id: w.id,
            name: w.name,
        })
        .collect())
}

/// Every workspace `user_id` can log into: the union of workspaces under every tenant they hold
/// a membership in. Used by `/auth/login` to resolve `workspace_id` automatically when the
/// caller can only reach one.
pub async fn list_workspaces_for_user(
    conn: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<Vec<WorkspaceSummary>, YorishiroError> {
    use crate::models::_entities::identity_workspaces;

    let memberships = identity_tenant_memberships::Entity::find()
        .filter(identity_tenant_memberships::Column::UserId.eq(user_id))
        .all(conn)
        .await
        .internal()?;

    let tenant_ids: Vec<Uuid> = memberships.into_iter().map(|m| m.tenant_id).collect();
    if tenant_ids.is_empty() {
        return Ok(vec![]);
    }

    let workspaces = identity_workspaces::Entity::find()
        .filter(identity_workspaces::Column::TenantId.is_in(tenant_ids))
        .all(conn)
        .await
        .internal()?;

    Ok(workspaces
        .into_iter()
        .map(|w| WorkspaceSummary {
            id: w.id,
            name: w.name,
        })
        .collect())
}

/// A workspace's id and owning tenant, for `/auth/login`'s explicit `workspace_id` path.
pub async fn get_workspace_tenant(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Uuid, YorishiroError> {
    use crate::models::_entities::identity_workspaces;

    identity_workspaces::Entity::find_by_id(workspace_id)
        .one(conn)
        .await
        .internal()?
        .map(|w| w.tenant_id)
        .ok_or_else(|| YorishiroError::not_found("workspace not found"))
}

#[derive(Debug, Serialize)]
pub struct UserRecord {
    pub id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<identity_users::Model> for UserRecord {
    fn from(m: identity_users::Model) -> Self {
        Self {
            id: m.id,
            email: m.email,
            display_name: m.display_name,
            created_at: m.created_at.into(),
        }
    }
}
