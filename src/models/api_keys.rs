//! API key model: key shapes and the authenticated context, with issuing, authentication and management in the submodules.

mod authenticate;
mod issue;
mod manage;

use aws_lc_rs::digest::{SHA256, digest};
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, FromQueryResult, QueryOrder, QuerySelect};

use crate::db::DbHandle;
use crate::error::{ResultExt, YorishiroError};
use crate::models::db_enum::db_enum;

const KEY_PREFIX_LENGTH: usize = 12;
const KEY_SECRET_LENGTH: usize = 48;
const LOGIN_SESSION_HOURS: i64 = 12;
// Ordinary key prefixes contain only random alphanumerics after `ysr_`.
const LOGIN_KEY_PREFIX: &str = "ysr_login-";

/// The request header that names which workspace a tenant-scoped key acts on.
pub const WORKSPACE_HEADER: &str = "x-workspace-id";

/// Secret-free grant metadata for credential administration.
#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct KeyGrant {
    pub state: KeyState,
    pub expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub revoked_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub replaced_by_id: Option<Uuid>,
    pub audit: bool,
    pub workspace_id: Option<Uuid>,
    pub tenant_id: Uuid,
    pub user_id: Option<Uuid>,
    pub identity_kind: &'static str,
}

impl Model {
    pub(crate) async fn is_root_credential(
        conn: &impl ConnectionTrait,
        id: Uuid,
    ) -> Result<bool, YorishiroError> {
        let key = Entity::find_by_id(id)
            .one(conn)
            .await
            .internal()?
            .filter(Self::is_active)
            .ok_or(YorishiroError::Unauthenticated)?;
        if !key.is_login_session() {
            return Ok(false);
        }
        key.has_root_principal(conn).await
    }

    pub(crate) fn is_login_session(&self) -> bool {
        self.key_prefix.starts_with(LOGIN_KEY_PREFIX)
    }

    pub(crate) async fn has_root_principal(
        &self,
        conn: &impl ConnectionTrait,
    ) -> Result<bool, YorishiroError> {
        match self.user_id {
            Some(user_id) => {
                crate::models::iam_tenant_roots::Model::contains(conn, self.tenant_id, user_id)
                    .await
            }
            None => Ok(false),
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.revoked_at.is_none()
            && self
                .expires_at
                .is_none_or(|expires| expires > crate::data::time::now())
    }

    pub(crate) async fn current_context(
        conn: &impl ConnectionTrait,
        presented: &AuthContext,
    ) -> Result<AuthContext, YorishiroError> {
        let key = Entity::find_by_id(presented.api_key_id)
            .one(conn)
            .await
            .internal()?
            .filter(|key| {
                key.is_active()
                    && key.tenant_id == presented.tenant_id
                    && key.user_id == presented.user_id
            })
            .ok_or(YorishiroError::Unauthenticated)?;
        if key
            .workspace_id
            .is_some_and(|id| Some(id) != presented.workspace_id)
        {
            return Err(YorishiroError::Unauthenticated);
        }
        if let Some(workspace_id) = presented.workspace_id {
            let tenant =
                crate::models::workspace_workspaces::get_workspace_tenant(conn, workspace_id)
                    .await?;
            if tenant != key.tenant_id {
                return Err(YorishiroError::Unauthenticated);
            }
        }
        AuthContext {
            api_key_id: key.id,
            workspace_id: presented.workspace_id,
            tenant_id: key.tenant_id,
            user_id: key.user_id,
            audit: key.audit,
        }
        .with_current_membership(conn)
        .await
    }

    pub(crate) fn grant_metadata(&self) -> KeyGrant {
        KeyGrant {
            state: if self.revoked_at.is_some() {
                KeyState::Revoked
            } else if !self.is_active() {
                KeyState::Expired
            } else {
                KeyState::Active
            },
            expires_at: self.expires_at,
            revoked_at: self.revoked_at,
            replaced_by_id: self.replaced_by_id,
            audit: self.audit,
            workspace_id: self.workspace_id,
            tenant_id: self.tenant_id,
            user_id: self.user_id,
            identity_kind: if self.user_id.is_some() {
                "user"
            } else {
                "service"
            },
        }
    }
}

db_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum KeyState {
        Active = "active",
        Revoked = "revoked",
        Expired = "expired",
    }
}

/// The workspace, tenant and user resolved by API key authentication.
#[derive(Clone)]
pub struct AuthContext {
    pub api_key_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub tenant_id: Uuid,
    /// The human user this key was issued for, if any.
    pub user_id: Option<Uuid>,
    /// A separate grant a key holds, checked with `require_audit`.
    pub audit: bool,
}

impl AuthContext {
    /// Requests already authenticated may finish; later authentications read the committed membership.
    pub(crate) async fn with_current_membership(
        mut self,
        conn: &impl ConnectionTrait,
    ) -> Result<Self, YorishiroError> {
        let key = Entity::find_by_id(self.api_key_id)
            .one(conn)
            .await
            .internal()?
            .filter(|key| {
                key.is_active() && key.tenant_id == self.tenant_id && key.user_id == self.user_id
            })
            .ok_or(YorishiroError::Unauthenticated)?;
        crate::models::tenant_tenants::Model::require_active(conn, self.tenant_id).await?;
        if let Some(user_id) = self.user_id
            && !crate::models::tenant_memberships::is_member(conn, self.tenant_id, user_id).await?
        {
            return Err(YorishiroError::Unauthenticated);
        }
        if key.is_login_session() && key.has_root_principal(conn).await? {
            self.audit = true;
        }
        Ok(self)
    }

    /// Requires the authorization or authentication flow to have resolved a workspace.
    ///
    /// # Errors
    /// Rejects tenant-only contexts.
    pub fn require_workspace(&self) -> Result<Uuid, YorishiroError> {
        self.workspace_id
            .ok_or_else(|| YorishiroError::ValidationFailed {
                message: "this operation requires a workspace".into(),
                details: Vec::new(),
                hint: format!("select a workspace with workspace_id or {WORKSPACE_HEADER}"),
            })
    }
}

pub struct CreatedApiKey {
    pub id: Uuid,
    /// `None` for tenant-scoped keys, which name their workspace per request instead.
    pub workspace_id: Option<Uuid>,
    pub(crate) grant: KeyGrant,
    pub(crate) key_prefix: String,
    pub(crate) name: String,
    pub(crate) created_at: chrono::DateTime<chrono::FixedOffset>,
    /// The raw API key string.
    /// Only its hash is stored in the DB, so this return value is the only place it can ever be obtained.
    pub plaintext: String,
}

/// A freshly issued tenant-scoped key.
/// The plaintext exists only here: only its hash is stored.
pub struct CreatedTenantApiKey {
    pub id: Uuid,
    pub plaintext: String,
}

/// Hashes a presented key into the form stored in `api_keys.key_hash`.
#[must_use]
pub fn hash_key(raw: &str) -> Vec<u8> {
    digest(&SHA256, raw.as_bytes()).as_ref().to_vec()
}

/// Generates the shared credential format; only the hash is persisted.
pub(crate) fn generate_key() -> (String, String) {
    generate_key_with_prefix("ysr_")
}

fn generate_key_with_prefix(kind: &str) -> (String, String) {
    let prefix = format!("{kind}{}", loco_rs::hash::random_string(KEY_PREFIX_LENGTH));
    let secret = loco_rs::hash::random_string(KEY_SECRET_LENGTH);
    let plaintext = format!("{prefix}_{secret}");
    (prefix, plaintext)
}

pub use crate::models::_entities::api_keys::{ActiveModel, Entity, Model};
pub(crate) type IdentityApiKeys = Entity;

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    /// `id` has a `uuidv7()` column default on PostgreSQL and no default on SQLite; see `crate::db::sqlite_generated_id`.
    async fn before_save<C>(mut self, db: &C, _insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        self.id = crate::db::sqlite_generated_id(db, self.id);
        Ok(self)
    }
}

/// Trims a key name and enforces its 1 to 100 character bound.
fn validate_key_name(name: &str) -> Result<String, YorishiroError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(YorishiroError::ValidationFailed {
            message: "name must be between 1 and 100 characters".into(),
            details: vec![crate::error::ValidationDetail {
                field: "name".into(),
                problem: "name must not be empty and must be at most 100 characters".into(),
                code: crate::error::ValidationErrorCode::EmptyRequired,
                expected: Some("1-100 characters".into()),
                actual: Some(name.chars().count().to_string()),
            }],
            hint: "provide a non-empty API key name".into(),
        });
    }
    Ok(name.to_owned())
}
