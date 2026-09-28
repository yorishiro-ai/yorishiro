//! Control-plane tenancy operations grouped by responsibility.
//!
//! This module keeps the historical `crate::models::tenancy` API stable while its implementations live in private modules.

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

/// The nil UUID, reserved for infrastructure tenants that own no members and no data of their own.
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
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }

    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    pub fn max_scope(self) -> ApiKeyScope {
        match self {
            Self::Owner | Self::Admin => ApiKeyScope::Migration,
            Self::Member => ApiKeyScope::Write,
            Self::Viewer => ApiKeyScope::Read,
        }
    }

    pub fn administers_tenant(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}

/// A tenant member as reported to a caller.
#[derive(Debug, Serialize)]
pub struct MembershipRecord {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub role: MembershipRole,
}

/// What a redeemed invite grants.
pub struct RedeemedInvite {
    pub tenant_id: Uuid,
    pub email: String,
    pub role: MembershipRole,
}

#[derive(Clone, Serialize)]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub name: String,
}

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

pub async fn count_tenants(conn: &impl ConnectionTrait) -> Result<u64, YorishiroError> {
    tenant::count_tenants(conn).await
}

pub async fn create_tenant(
    conn: &impl ConnectionTrait,
    name: &str,
) -> Result<tenant_tenants::Model, YorishiroError> {
    tenant::create_tenant(conn, name).await
}

pub fn max_tenants_from_env() -> Result<Option<i32>, YorishiroError> {
    tenant::max_tenants_from_env()
}

pub async fn create_user(
    conn: &impl ConnectionTrait,
    email: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<user_users::Model, YorishiroError> {
    users::create_user(conn, email, password, display_name).await
}

pub async fn verify_login(
    conn: &impl ConnectionTrait,
    email: &str,
    password: &str,
) -> Result<Option<user_users::Model>, YorishiroError> {
    users::verify_login(conn, email, password).await
}

pub async fn add_member(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<(), YorishiroError> {
    membership::add_member(conn, tenant_id, user_id, role).await
}

pub async fn get_user_by_email(
    conn: &impl ConnectionTrait,
    email: &str,
) -> Result<Option<user_users::Model>, YorishiroError> {
    users::get_user_by_email(conn, email).await
}

pub async fn list_members(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<MembershipRecord>, YorishiroError> {
    membership::list_members(conn, tenant_id, page).await
}

pub async fn get_membership_role(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<MembershipRole>, YorishiroError> {
    membership::get_membership_role(conn, tenant_id, user_id).await
}

pub async fn create_invite(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    email: &str,
    role: MembershipRole,
    ttl: Duration,
) -> Result<(workspace_invites::Model, String), YorishiroError> {
    invite::create_invite(conn, tenant_id, email, role, ttl).await
}

pub async fn redeem_invite(
    conn: &impl ConnectionTrait,
    raw_token: &str,
) -> Result<Option<RedeemedInvite>, YorishiroError> {
    invite::redeem_invite(conn, raw_token).await
}

pub async fn list_workspaces(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<WorkspaceSummary>, YorishiroError> {
    workspace::list_workspaces(conn, tenant_id, page).await
}

pub async fn list_workspaces_for_user(
    conn: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<Vec<WorkspaceSummary>, YorishiroError> {
    orchestration::list_workspaces_for_user(conn, user_id).await
}

pub async fn get_workspace_tenant(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Uuid, YorishiroError> {
    workspace::get_workspace_tenant(conn, workspace_id).await
}

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

pub async fn set_tenant_max_workspaces(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    max_workspaces: Option<i32>,
) -> Result<(), YorishiroError> {
    tenant::set_tenant_max_workspaces(conn, tenant_id, max_workspaces).await
}

pub async fn get_workspace(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<workspace_workspaces::Model, YorishiroError> {
    workspace::get_workspace(conn, workspace_id).await
}

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
