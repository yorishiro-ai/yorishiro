//! Control-plane tenancy operations for signup, login, invites, memberships, and workspaces.
//!
//! These operations use Loco's control-plane connection rather than the RLS-scoped tenant pool because a new tenant has no workspace context yet.
//! This module keeps the historical `crate::models::identity::tenancy` API stable while its implementations live in private modules.

use chrono::{DateTime, Duration, Utc};
use sea_orm::{ConnectionTrait, DatabaseTransaction};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::_entities::{
    tenant_tenants, user_users, workspace_invites, workspace_workspaces,
};
use crate::services::auth::ApiKeyScope;

#[path = "tenancy/invite.rs"]
mod invite;
#[path = "tenancy/membership.rs"]
mod membership;
#[path = "tenancy/orchestration.rs"]
mod orchestration;
#[path = "tenancy/tenant.rs"]
mod tenant;
#[path = "tenancy/users.rs"]
mod users;
#[path = "tenancy/workspace.rs"]
mod workspace;

/// The nil UUID reserved for infrastructure tenants that own no members and no data of their own.
/// It is excluded from tenant-limit counts.
pub const INFRASTRUCTURE_TENANT_ID: Uuid = Uuid::nil();

/// Mirrors the `tenant_memberships.role` check constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MembershipRole {
    Owner,
    Admin,
    Member,
    Viewer,
}

impl MembershipRole {
    /// Returns the database representation of this role.
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }

    /// Parses a database role representation.
    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    /// Returns the highest API key scope a member with this role may be issued.
    pub fn max_scope(self) -> ApiKeyScope {
        match self {
            Self::Owner | Self::Admin => ApiKeyScope::Migration,
            Self::Member => ApiKeyScope::Write,
            Self::Viewer => ApiKeyScope::Read,
        }
    }

    /// Returns whether this role may manage the tenant and its workspaces.
    pub fn administers_tenant(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}

/// A tenant member as reported by member-list and member-add operations.
#[derive(Debug, Serialize)]
pub struct MembershipRecord {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub role: MembershipRole,
}

/// The tenant, email, and role granted by a redeemed invite.
pub struct RedeemedInvite {
    pub tenant_id: Uuid,
    pub email: String,
    pub role: MembershipRole,
}

/// A workspace summary returned by tenancy lookups and signup.
#[derive(Clone, Serialize)]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub name: String,
}

/// A user account returned by tenancy operations.
#[derive(Serialize)]
pub struct UserRecord {
    pub id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<user_users::Model> for UserRecord {
    fn from(model: user_users::Model) -> Self {
        Self {
            id: model.id,
            email: model.email,
            display_name: model.display_name,
            created_at: model.created_at.into(),
        }
    }
}

/// Counts real tenants and excludes `INFRASTRUCTURE_TENANT_ID`.
pub async fn count_tenants(conn: &impl ConnectionTrait) -> Result<u64, YorishiroError> {
    tenant::count_tenants(conn).await
}

/// Creates a tenant and enforces the configured tenant cap.
/// On PostgreSQL, `conn` must be a transaction because the count and insert are protected by `db::lock_for_update`.
/// On SQLite, the cap is hardcoded to one because the backend has no database-enforced tenant isolation, regardless of `YORISHIRO_MAX_TENANTS`.
/// On SQLite, the generated tenant ID is supplied by the ActiveModel hook rather than by this function.
pub async fn create_tenant(
    conn: &impl ConnectionTrait,
    name: &str,
    configured_max: Option<i32>,
) -> Result<tenant_tenants::Model, YorishiroError> {
    tenant::create_tenant(conn, name, configured_max).await
}

#[cfg(feature = "enterprise")]
pub(crate) fn max_tenants_from_env() -> Result<Option<i32>, YorishiroError> {
    match std::env::var("YORISHIRO_MAX_TENANTS") {
        Ok(raw) => {
            let parsed = raw.parse::<i32>().map_err(|_| {
                YorishiroError::Internal(anyhow::anyhow!(
                    "YORISHIRO_MAX_TENANTS must be an integer, got '{raw}'"
                ))
            })?;
            match parsed {
                0 => Ok(None),
                n if n < 0 => Err(YorishiroError::Internal(anyhow::anyhow!(
                    "YORISHIRO_MAX_TENANTS must not be negative, got '{raw}'"
                ))),
                n => Ok(Some(n)),
            }
        }
        Err(_) => Ok(None),
    }
}

/// Creates a human user account.
/// The password is hashed with Argon2id before it reaches the database.
/// The `ConnectionTrait` boundary lets callers compose user creation with membership insertion in one transaction.
pub async fn create_user(
    conn: &impl ConnectionTrait,
    email: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<user_users::Model, YorishiroError> {
    users::create_user(conn, email, password, display_name).await
}

/// Verifies an email and password against the stored Argon2id hash.
/// Accounts without a password hash never match.
pub async fn verify_login(
    conn: &impl ConnectionTrait,
    email: &str,
    password: &str,
) -> Result<Option<user_users::Model>, YorishiroError> {
    users::verify_login(conn, email, password).await
}

/// Adds a user's membership or updates the role of an existing membership.
/// The `ConnectionTrait` boundary lets callers compose the upsert with other writes in one transaction.
/// The upsert supplies SQLite's generated membership ID explicitly because conflict inserts bypass the ActiveModel hook.
pub async fn add_member(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<(), YorishiroError> {
    membership::add_member(conn, tenant_id, user_id, role).await
}

/// Looks up an existing user by email without creating an account.
pub async fn get_user_by_email(
    conn: &impl ConnectionTrait,
    email: &str,
) -> Result<Option<user_users::Model>, YorishiroError> {
    users::get_user_by_email(conn, email).await
}

/// Lists the members of a tenant using the supplied pagination parameters.
pub async fn list_members(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<MembershipRecord>, YorishiroError> {
    membership::list_members(conn, tenant_id, page).await
}

/// Looks up a user's role within a tenant.
pub async fn get_membership_role(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<MembershipRole>, YorishiroError> {
    membership::get_membership_role(conn, tenant_id, user_id).await
}

/// Creates an invite for the requested role and TTL and returns its record together with the plaintext token.
/// Only the token hash is persisted, and the plaintext is available only to this caller.
pub async fn create_invite(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    email: &str,
    role: MembershipRole,
    ttl: Duration,
) -> Result<(workspace_invites::Model, String), YorishiroError> {
    invite::create_invite(conn, tenant_id, email, role, ttl).await
}

/// Redeems an invite if its token is valid, unused, and unexpired.
/// Redemption marks the invite used atomically, so concurrent attempts cannot both succeed.
pub async fn redeem_invite(
    conn: &impl ConnectionTrait,
    raw_token: &str,
) -> Result<Option<RedeemedInvite>, YorishiroError> {
    invite::redeem_invite(conn, raw_token).await
}

/// Lists a tenant's workspaces using the supplied pagination parameters.
pub async fn list_workspaces(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<WorkspaceSummary>, YorishiroError> {
    workspace::list_workspaces(conn, tenant_id, page).await
}

/// Lists every workspace a user can log into across all of the user's tenant memberships.
/// This lookup is deliberately unpaginated because login must distinguish exactly one workspace from multiple workspaces.
pub async fn list_workspaces_for_user(
    conn: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<Vec<WorkspaceSummary>, YorishiroError> {
    orchestration::list_workspaces_for_user(conn, user_id).await
}

/// Returns the tenant that owns a workspace for the explicit workspace-login path.
pub async fn get_workspace_tenant(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Uuid, YorishiroError> {
    workspace::get_workspace_tenant(conn, workspace_id).await
}

/// Creates a workspace under a tenant and enforces its `max_workspaces` cap, where `None` means unlimited.
/// On PostgreSQL, `conn` must be a `DatabaseTransaction` because the per-tenant advisory lock is transaction-scoped.
/// Creation and deletion share that lock so their count-and-write decisions serialize with each other.
/// The workspace status is `Active` when `schema_id` is present and `SchemaPending` otherwise.
/// The supplied embedding model and dimensions are not stamped at creation, and the workspace starts with null embedding metadata.
/// The first successful embedding write stamps its model and dimensions, avoiding a sentinel stamp when no provider is configured.
pub async fn create_workspace(
    conn: &DatabaseTransaction,
    tenant_id: Uuid,
    name: &str,
    max_entities: Option<i32>,
    schema_id: Option<Uuid>,
    embedding: Option<(&str, i32)>,
) -> Result<workspace_workspaces::Model, YorishiroError> {
    orchestration::create_workspace(conn, tenant_id, name, max_entities, schema_id, embedding).await
}

/// Sets a tenant's optional `max_workspaces` cap.
pub async fn set_tenant_max_workspaces(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    max_workspaces: Option<i32>,
) -> Result<(), YorishiroError> {
    tenant::set_tenant_max_workspaces(conn, tenant_id, max_workspaces).await
}

/// Fetches a workspace by ID.
pub async fn get_workspace(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<workspace_workspaces::Model, YorishiroError> {
    workspace::get_workspace(conn, workspace_id).await
}

/// Deletes a workspace while refusing to remove a tenant's last remaining workspace.
/// On PostgreSQL, `conn` must be a `DatabaseTransaction` because deletion uses the same transaction-scoped per-tenant advisory lock as workspace creation.
/// The shared lock serializes the count and delete with concurrent workspace creation and deletion for the same tenant.
pub async fn delete_workspace(
    conn: &DatabaseTransaction,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    workspace::delete_workspace(conn, workspace_id).await
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub mod test_support {
    use super::*;

    pub async fn create_invite_at(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        email: &str,
        role: MembershipRole,
        ttl: Duration,
        now: DateTime<Utc>,
    ) -> Result<(workspace_invites::Model, String), YorishiroError> {
        invite::test_support::create_invite_at(conn, tenant_id, email, role, ttl, now).await
    }

    pub async fn redeem_invite_at(
        conn: &impl ConnectionTrait,
        raw_token: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<RedeemedInvite>, YorishiroError> {
        invite::test_support::redeem_invite_at(conn, raw_token, now).await
    }
}
