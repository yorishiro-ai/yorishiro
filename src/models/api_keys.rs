//! API key model: key shapes, scopes and the authenticated context, with issuing, authentication and management in the submodules.

mod authenticate;
mod issue;
mod manage;

use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, FromQueryResult, QueryOrder, QuerySelect};
use sha2::{Digest, Sha256};

use crate::db::{AppContextBackend, DbHandle};
use crate::error::{ResultExt, YorishiroError};
use crate::models::db_enum::db_enum;

const KEY_PREFIX_LENGTH: usize = 12;
const KEY_SECRET_LENGTH: usize = 48;
const LOGIN_SESSION_HOURS: i64 = 12;

/// The request header that names which workspace a tenant-scoped key acts on.
pub const WORKSPACE_HEADER: &str = "x-workspace-id";

/// Secret-free grant metadata for credential administration.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct KeyGrant {
    pub state: KeyState,
    pub expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub revoked_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub replaced_by_id: Option<Uuid>,
    pub scope: ApiKeyScope,
    pub effective_scope: Option<ApiKeyScope>,
    pub audit: bool,
    pub workspace_id: Option<Uuid>,
    pub tenant_id: Uuid,
    pub user_id: Option<Uuid>,
    pub identity_kind: &'static str,
}

impl Model {
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
                    && key
                        .workspace_id
                        .is_none_or(|id| Some(id) == presented.workspace_id)
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
            api_key_id: key.id,
            workspace_id: presented.workspace_id,
            tenant_id: key.tenant_id,
            user_id: key.user_id,
            scope: stored_scope(&key.scope)?,
            audit: key.audit,
        }
        .with_current_membership(conn)
        .await
    }

    pub(crate) async fn grant_metadata(
        &self,
        conn: &impl ConnectionTrait,
    ) -> Result<KeyGrant, YorishiroError> {
        let scope = stored_scope(&self.scope)?;
        let effective_scope = if self.is_active() {
            let presented = AuthContext {
                api_key_id: self.id,
                workspace_id: self.workspace_id,
                tenant_id: self.tenant_id,
                user_id: self.user_id,
                scope,
                audit: self.audit,
            };
            match Self::current_context(conn, &presented).await {
                Ok(current) => Some(current.scope),
                Err(YorishiroError::Unauthenticated | YorishiroError::ScopeInsufficient { .. }) => {
                    None
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        Ok(KeyGrant {
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
            scope,
            effective_scope,
            audit: self.audit,
            workspace_id: self.workspace_id,
            tenant_id: self.tenant_id,
            user_id: self.user_id,
            identity_kind: if self.user_id.is_some() {
                "user"
            } else {
                "service"
            },
        })
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

db_enum! {
    /// Permission level held by an API key.
    /// Declaration order feeds the derived `Ord`: `Read < Write < Schema < Migration`, a higher scope subsumes lower ones.
    /// The wire form matches the DB `scope` column ('read'/'write'/'schema'/'migration').
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, schemars::JsonSchema)]
    #[schemars(rename_all = "lowercase")]
    pub enum ApiKeyScope {
        Read = "read",
        Write = "write",
        Schema = "schema",
        /// Running a batch migration, and switching maintenance mode.
        /// Above `schema` because both act on data already stored.
        Migration = "migration",
    }
}

impl ApiKeyScope {
    /// Whether a key with this scope can perform an operation requiring `required`.
    #[must_use]
    pub fn satisfies(self, required: Self) -> bool {
        self >= required
    }
}

/// Workspace, tenant, and scope information resolved by API key authentication.
#[derive(Clone)]
pub struct AuthContext {
    pub api_key_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub tenant_id: Uuid,
    pub scope: ApiKeyScope,
    /// The human user this key was issued for, if any.
    pub user_id: Option<Uuid>,
    /// Independent of `scope`: not one more rung above `Migration` on the read/write/schema/migration ladder, but a separate grant a key holds alongside whatever `scope` it has.
    /// Checked with `require_audit`, never through `ApiKeyScope`'s `Ord`/`satisfies`.
    pub audit: bool,
}

impl AuthContext {
    /// Requests already authenticated may finish; later authentications read the committed membership.
    pub(crate) async fn with_current_membership(
        self,
        conn: &impl ConnectionTrait,
    ) -> Result<Self, YorishiroError> {
        crate::models::tenant_tenants::Model::require_active(conn, self.tenant_id).await?;
        if let Some(user_id) = self.user_id
            && !crate::models::tenant_memberships::is_member(conn, self.tenant_id, user_id).await?
        {
            return Err(YorishiroError::Unauthenticated);
        }
        Ok(self)
    }

    /// Requires an explicitly selected workspace.
    ///
    /// # Errors
    /// Rejects tenant-only contexts.
    pub fn require_workspace(&self) -> Result<Uuid, YorishiroError> {
        self.workspace_id
            .ok_or_else(|| YorishiroError::ValidationFailed {
                message: format!("this operation requires {WORKSPACE_HEADER}"),
                details: Vec::new(),
                hint: "select a workspace or use a workspace-bound key".into(),
            })
    }
}

pub struct CreatedApiKey {
    pub id: Uuid,
    /// `None` for tenant-scoped keys, which name their workspace per request instead.
    pub workspace_id: Option<Uuid>,
    pub scope: ApiKeyScope,
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
    Sha256::digest(raw.as_bytes()).to_vec()
}

/// Generates the shared credential format; only the hash is persisted.
pub(crate) fn generate_key() -> (String, String) {
    let prefix = format!("ysr_{}", loco_rs::hash::random_string(KEY_PREFIX_LENGTH));
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

/// A stored scope this crate does not define is a corrupt row, not a missing scope.
fn stored_scope(value: &str) -> Result<ApiKeyScope, YorishiroError> {
    ApiKeyScope::from_db_str(value).ok_or_else(|| {
        YorishiroError::Internal(format!("unknown api key scope in database: {value}").into())
    })
}
