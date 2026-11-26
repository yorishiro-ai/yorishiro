//! access key model: key shapes and the authenticated context, with issuing, authentication and management in the submodules.

mod authenticate;
mod issue;
mod manage;

use aws_lc_rs::digest::{SHA256, digest};
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, FromQueryResult, QueryOrder};

use crate::db::DbHandle;
use crate::error::{ResultExt, YorishiroError};
use crate::models::db_enum::db_enum;

const KEY_PREFIX_LENGTH: usize = 12;
const KEY_SECRET_LENGTH: usize = 48;
const LOGIN_SESSION_HOURS: i64 = 12;
// Ordinary key prefixes contain only random alphanumerics after `ysr_`.
const LOGIN_KEY_PREFIX: &str = "ysr_login-";

/// The request header that names the workspace a request acts on.
pub const WORKSPACE_HEADER: &str = "x-workspace-id";

/// Secret-free grant metadata for credential administration.
#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct KeyGrant {
    pub state: KeyState,
    pub expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub revoked_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub enabled: bool,
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
            && self.enabled
            && self
                .expires_at
                .is_none_or(|expires| expires > crate::data::time::now())
    }

    pub(crate) async fn current_context(
        conn: &impl ConnectionTrait,
        presented: &AuthContext,
    ) -> Result<AuthContext, YorishiroError> {
        let key = Entity::find_by_id(presented.access_key_id)
            .one(conn)
            .await
            .internal()?
            .filter(|key| {
                key.is_active()
                    && key.tenant_id == presented.tenant_id
                    && key.user_id == presented.user_id
            })
            .ok_or(YorishiroError::Unauthenticated)?;
        if let Some(workspace_id) = presented.workspace_id {
            let tenant =
                crate::models::workspace_workspaces::get_workspace_tenant(conn, workspace_id)
                    .await?;
            if tenant != key.tenant_id {
                return Err(YorishiroError::Unauthenticated);
            }
        }
        AuthContext {
            access_key_id: key.id,
            workspace_id: presented.workspace_id,
            tenant_id: key.tenant_id,
            user_id: key.user_id,
        }
        .with_current_membership(conn)
        .await
    }

    pub(crate) fn grant_metadata(&self) -> KeyGrant {
        KeyGrant {
            state: if self.revoked_at.is_some() {
                KeyState::Revoked
            } else if !self.enabled {
                KeyState::Disabled
            } else if !self.is_active() {
                KeyState::Expired
            } else {
                KeyState::Active
            },
            expires_at: self.expires_at,
            revoked_at: self.revoked_at,
            enabled: self.enabled,
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
        Disabled = "disabled",
        Expired = "expired",
    }
}

/// The workspace, tenant and user resolved by access key authentication.
#[derive(Clone)]
#[expect(
    clippy::struct_field_names,
    reason = "each field names the resource it identifies"
)]
pub struct AuthContext {
    pub access_key_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub tenant_id: Uuid,
    /// The human user this key was issued for, if any.
    pub user_id: Option<Uuid>,
}

impl AuthContext {
    /// The YRN of the access key, which every log line about the caller carries.
    #[must_use]
    pub fn yrn(&self) -> String {
        crate::models::iam::names::access_key_yrn(self.tenant_id, self.access_key_id)
    }

    /// The YRN of the selected workspace, if one is selected.
    #[must_use]
    pub fn workspace_yrn(&self) -> Option<String> {
        self.workspace_id.map(|workspace_id| {
            crate::models::iam::policy::Resource::Workspace {
                tenant_id: self.tenant_id,
                workspace_id,
            }
            .yrn()
        })
    }

    /// Requests already authenticated may finish; later authentications read the committed membership.
    pub(crate) async fn with_current_membership(
        mut self,
        conn: &impl ConnectionTrait,
    ) -> Result<Self, YorishiroError> {
        let _key = Entity::find_by_id(self.access_key_id)
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
        if self.workspace_id.is_none() {
            self.workspace_id = Self::sole_workspace(conn, self.tenant_id).await?;
        }
        Ok(self)
    }

    /// The tenant's only workspace, which a request without a selection acts on; `None` when the tenant has several.
    async fn sole_workspace(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
    ) -> Result<Option<Uuid>, YorishiroError> {
        use crate::models::_entities::workspace_workspaces::{Column, Entity};
        use sea_orm::QuerySelect;
        let ids: Vec<Uuid> = Entity::find()
            .select_only()
            .column(Column::Id)
            .filter(Column::TenantId.eq(tenant_id))
            .limit(2)
            .into_tuple()
            .all(conn)
            .await
            .internal()?;
        Ok(match ids.as_slice() {
            [only] => Some(*only),
            _ => None,
        })
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

/// The fields `update_access_key` may change; `None` leaves a field as it is.
pub(crate) struct AccessKeyChange {
    pub(crate) name: Option<String>,
    pub(crate) expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// Applies `expires_at` even when it is `None`, which clears the expiry.
    pub(crate) replace_expires_at: bool,
    pub(crate) enabled: Option<bool>,
}

pub struct CreatedAccessKey {
    pub id: Uuid,
    pub(crate) grant: KeyGrant,
    pub(crate) key_prefix: String,
    pub(crate) name: String,
    pub(crate) created_at: chrono::DateTime<chrono::FixedOffset>,
    /// The raw access key string.
    /// Only its hash is stored in the DB, so this return value is the only place it can ever be obtained.
    pub plaintext: String,
}

/// A freshly issued key for a tenant.
/// The plaintext exists only here: only its hash is stored.
pub struct CreatedTenantAccessKey {
    pub id: Uuid,
    pub plaintext: String,
}

/// Hashes a presented key into the form stored in `access_keys.key_hash`.
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

pub use crate::models::_entities::access_keys::{ActiveModel, Entity, Model};
pub(crate) type IdentityAccessKeys = Entity;

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::generated_id(this.id);
        this.created_at = crate::db::stamped_created_at(insert, this.created_at);
        Ok(this)
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
            hint: "provide a non-empty access key name".into(),
        });
    }
    Ok(name.to_owned())
}
