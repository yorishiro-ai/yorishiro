use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, FromQueryResult, QueryOrder, QuerySelect};
use sha2::{Digest, Sha256};

use crate::db::{AppContextBackend, DbHandle};
use crate::db_enum::db_enum;
use crate::error::{ResultExt, YorishiroError};

const KEY_PREFIX_LENGTH: usize = 12;
const KEY_SECRET_LENGTH: usize = 48;

/// Secret-free grant metadata for credential administration.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct KeyGrant {
    pub state: KeyState,
    pub expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub revoked_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub replaced_by_id: Option<Uuid>,
    pub scope: ApiKeyScope,
    /// None when no workspace target is selected or the attributed user has no access.
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
                .is_none_or(|expires| expires > crate::time::now())
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
                        .is_none_or(|id| id == presented.workspace_id)
            })
            .ok_or(YorishiroError::Unauthenticated)?;
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
        let effective_scope = if !self.is_active() {
            None
        } else {
            match self.user_id {
                None => Some(scope),
                Some(user_id) => match self.workspace_id {
                    Some(workspace_id) => crate::models::access::workspace_access(
                        conn,
                        self.tenant_id,
                        workspace_id,
                        user_id,
                    )
                    .await?
                    .map(|access| scope.min(access.max_scope)),
                    None => None,
                },
            }
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
    pub fn satisfies(self, required: ApiKeyScope) -> bool {
        self >= required
    }
}

/// Workspace, tenant, and scope information resolved by API key authentication.
#[derive(Clone)]
pub struct AuthContext {
    pub api_key_id: Uuid,
    pub workspace_id: Uuid,
    pub tenant_id: Uuid,
    pub scope: ApiKeyScope,
    /// The human user this key was issued for, if any.
    pub user_id: Option<Uuid>,
    /// Independent of `scope`: not one more rung above `Migration` on the read/write/schema/migration ladder, but a separate grant a key holds alongside whatever `scope` it has.
    /// Checked with `require_audit`, never through `ApiKeyScope`'s `Ord`/`satisfies`.
    pub audit: bool,
}

impl AuthContext {
    /// Applies current membership without widening the key's stored grant.
    /// Requests already authenticated may finish; later authentications read the committed membership.
    pub(crate) async fn with_current_membership(
        mut self,
        conn: &impl ConnectionTrait,
    ) -> Result<Self, YorishiroError> {
        crate::models::tenant_tenants::Model::require_active(conn, self.tenant_id).await?;
        if let Some(user_id) = self.user_id {
            crate::models::tenant_memberships::get_membership_role(conn, self.tenant_id, user_id)
                .await?
                .ok_or(YorishiroError::Unauthenticated)?;
            let access = crate::models::access::workspace_access(
                conn,
                self.tenant_id,
                self.workspace_id,
                user_id,
            )
            .await?
            .ok_or_else(|| YorishiroError::ScopeInsufficient {
                message: "this account has no access to the selected workspace".into(),
                hint: "ask a tenant administrator for a workspace grant".into(),
            })?;
            self.scope = self.scope.min(access.max_scope);
        }
        Ok(self)
    }
}

pub struct CreatedApiKey {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub scope: ApiKeyScope,
    pub(crate) grant: KeyGrant,
    pub(crate) key_prefix: String,
    pub(crate) name: String,
    pub(crate) created_at: chrono::DateTime<chrono::FixedOffset>,
    /// The raw API key string.
    /// Only its hash is stored in the DB, so this return value is the only place it can ever be obtained.
    pub plaintext: String,
}

/// Hashes a presented key into the form stored in `api_keys.key_hash`.
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
    async fn before_save<C>(mut self, db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        self.id = crate::db::sqlite_generated_id(db, self.id);
        Ok(self)
    }
}

impl Entity {
    /// Verifies a presented raw API key and resolves the workspace, tenant, and scope it belongs to.
    ///
    /// At this point neither the workspace nor the tenant is known yet, so RLS's `app.current_workspace`/`app.current_tenant` can't be set, which is why this takes the whole [`DbHandle`] rather than a scoped connection.
    ///
    /// SeaQuery calls the `authenticate_api_key` SECURITY DEFINER function to read through RLS and projects only id/workspace_id/tenant_id/scope/user_id/audit, never key_hash itself.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn authenticate(
        db: &DbHandle,
        presented_key: &str,
    ) -> Result<AuthContext, YorishiroError> {
        use sea_orm::sea_query::{Alias, Expr, Func, Query};
        #[derive(sea_orm::FromQueryResult)]
        struct Credential {
            id: uuid::Uuid,
            workspace_id: uuid::Uuid,
            tenant_id: uuid::Uuid,
            scope: String,
            user_id: Option<uuid::Uuid>,
            audit: bool,
        }
        let query = Query::select()
            .columns(
                [
                    "id",
                    "workspace_id",
                    "tenant_id",
                    "scope",
                    "user_id",
                    "audit",
                ]
                .map(Alias::new),
            )
            .from_function(
                Func::cust(Alias::new("authenticate_api_key"))
                    .arg(Expr::val(hash_key(presented_key))),
                Alias::new("credential"),
            )
            .to_owned();
        let conn =
            sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(db.tenant.pool().clone());
        let row = Credential::find_by_statement(conn.get_database_backend().build(&query))
            .one(&conn)
            .await
            .internal()?;

        let row = row.ok_or(YorishiroError::Unauthenticated)?;
        AuthContext {
            api_key_id: row.id,
            workspace_id: row.workspace_id,
            tenant_id: row.tenant_id,
            scope: stored_scope(&row.scope)?,
            user_id: row.user_id,
            audit: row.audit,
        }
        .with_current_membership(&db.identity_connection())
        .await
    }

    /// SQLite equivalent of [`Self::authenticate`], for a deployment with no `DbHandle` (see `Hooks::after_context`).
    ///
    /// The Postgres path goes through the SECURITY DEFINER function specifically to read rows RLS would otherwise hide from an unauthenticated caller; SQLite has no RLS at all, so there is nothing to bypass, and this queries the tables directly with the SeaORM entity API.
    /// Matches the SQL function's single-argument overload exactly: only a workspace-scoped key (`workspace_id` set) resolves; a tenant-scoped key matches nothing here either, same as on Postgres.
    ///
    /// The default authenticator selects this implementation for SQLite.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn authenticate_sqlite(
        conn: &impl ConnectionTrait,
        presented_key: &str,
    ) -> Result<AuthContext, YorishiroError> {
        let (key, tenant_id) = Self::find_sqlite_auth_context(conn, hash_key(presented_key))
            .await?
            .ok_or(YorishiroError::Unauthenticated)?;
        let workspace_id = key.workspace_id.ok_or(YorishiroError::Unauthenticated)?;
        AuthContext {
            api_key_id: key.id,
            workspace_id,
            tenant_id,
            scope: stored_scope(&key.scope)?,
            user_id: key.user_id,
            audit: key.audit,
        }
        .with_current_membership(conn)
        .await
    }

    /// Records authenticated usage outside the request transaction, preserving read-only requests' timestamps.
    /// Connection failures are best-effort on both backends and never reject a valid credential.
    pub(crate) async fn touch_authenticated(
        app: &loco_rs::app::AppContext,
        auth: &AuthContext,
    ) -> Result<(), YorishiroError> {
        if app.is_sqlite() {
            Self::touch_last_used(&app.db, auth.api_key_id).await;
            return Ok(());
        }
        let db = app
            .shared_store
            .get::<DbHandle>()
            .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
        match db
            .tenant
            .acquire_for_workspace(auth.tenant_id, auth.workspace_id)
            .await
        {
            Ok(mut conn) => {
                if let Err(error) =
                    Self::touch_last_used_on_connection(conn.as_mut(), auth.api_key_id).await
                {
                    tracing::warn!(%error, "failed to update api key last_used_at");
                }
            }
            Err(error) => {
                tracing::warn!(%error, "failed to acquire connection to touch last_used_at");
            }
        }
        Ok(())
    }

    /// Records the key's last-used timestamp on a raw `sqlx` connection.
    /// Best-effort: doesn't affect authentication outcomes, so callers don't need to fail the whole request if it errors.
    ///
    /// Deliberately not run on the request's `DatabaseTransaction`: a read-only handler drops that transaction without committing, which would silently roll this update back along with it.
    /// Every caller uses a short-lived connection from `TenantDb::acquire_for_workspace` instead.
    ///
    /// This call receives a bare RLS-scoped `PgConnection`, not a SeaORM `ConnectionTrait`, so the write must use `sqlx` on that connection.
    pub(crate) async fn touch_last_used_on_connection(
        conn: &mut sea_orm::sqlx::PgConnection,
        api_key_id: uuid::Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::_entities::api_keys::Column;
        use sea_orm::sea_query::{Expr, ExprTrait, Func, PostgresQueryBuilder, Query};
        let (sql, _) = Query::update()
            .table(Entity)
            .value(
                Column::LastUsedAt,
                Func::cust(sea_orm::sea_query::Alias::new("now")),
            )
            .and_where(Expr::col(Column::Id).eq(api_key_id))
            .build(PostgresQueryBuilder);
        sea_orm::sqlx::query(sea_orm::sqlx::AssertSqlSafe(sql))
            .bind(api_key_id)
            .execute(conn)
            .await
            .internal()?;
        Ok(())
    }

    pub(crate) async fn find_sqlite_auth_context(
        conn: &impl ConnectionTrait,
        key_hash: Vec<u8>,
    ) -> Result<Option<(Model, uuid::Uuid)>, YorishiroError> {
        use crate::models::_entities::api_keys::Column;

        let Some(key) = Entity::find()
            .filter(Column::KeyHash.eq(key_hash))
            .one(conn)
            .await
            .internal()?
        else {
            return Ok(None);
        };
        if !key.is_active() {
            return Ok(None);
        }
        let Some(workspace_id) = key.workspace_id else {
            return Ok(None);
        };
        let tenant_id = crate::models::workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(conn)
            .await
            .internal()?
            .map(|workspace| workspace.tenant_id);
        Ok(tenant_id.map(|tenant_id| (key, tenant_id)))
    }

    pub(crate) async fn touch_last_used(conn: &impl ConnectionTrait, api_key_id: uuid::Uuid) {
        use crate::models::_entities::api_keys::Column;

        if let Err(err) = Entity::update_many()
            .col_expr(
                Column::LastUsedAt,
                sea_orm::sea_query::Expr::value(crate::time::now()),
            )
            .filter(Column::Id.eq(api_key_id))
            .exec(conn)
            .await
        {
            tracing::warn!(error = %err, "failed to update api key last_used_at");
        }
    }

    /// Issues a new API key of the form `ysr_<prefix>_<secret>`, with 48 random alphanumeric secret characters (over 285 bits of entropy).
    /// SHA-256 is sufficient here rather than a slow KDF like bcrypt/argon2, since API keys already carry enough entropy that offline brute-forcing isn't a realistic threat.
    /// `audit` is independent of `scope`: it does not raise or lower where the key sits on the read/write/schema/migration ladder, only whether it additionally holds the separate grant `AuthContext::audit`'s doc comment describes.
    /// Every caller except the `create_api_key` CLI task passes `false`: an audit-reading key is an explicit operator decision, never a side effect of signup, login, or any other flow provisioning a key for an ordinary user.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn create_api_key(
        db: &sea_orm::DatabaseConnection,
        workspace_id: uuid::Uuid,
        scope: ApiKeyScope,
        user_id: Option<uuid::Uuid>,
        audit: bool,
    ) -> Result<CreatedApiKey, YorishiroError> {
        Self::create_named_api_key(
            db,
            workspace_id,
            scope,
            user_id,
            audit,
            "unnamed",
            None,
            None,
        )
        .await
    }

    pub(crate) async fn create_delegated_api_key(
        db: &sea_orm::DatabaseConnection,
        issuer: &AuthContext,
        scope: ApiKeyScope,
        audit: bool,
        name: &str,
        expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    ) -> Result<CreatedApiKey, YorishiroError> {
        Self::create_named_api_key(
            db,
            issuer.workspace_id,
            scope,
            issuer.user_id,
            audit,
            name,
            Some(issuer),
            expires_at,
        )
        .await
    }

    /// Issues a key and validates attribution while holding the membership fence.
    #[allow(clippy::too_many_arguments)]
    async fn create_named_api_key(
        db: &sea_orm::DatabaseConnection,
        workspace_id: uuid::Uuid,
        scope: ApiKeyScope,
        user_id: Option<uuid::Uuid>,
        audit: bool,
        name: &str,
        issuer: Option<&AuthContext>,
        expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    ) -> Result<CreatedApiKey, YorishiroError> {
        use crate::models::_entities::workspace_workspaces;

        let txn = crate::db::begin_write(db).await.internal()?;
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
        if expires_at.is_some_and(|expires| expires <= crate::time::now()) {
            return Err(YorishiroError::ValidationFailed {
                message: "key expiry must be in the future".into(),
                details: vec![],
                hint: "choose a future expires_at or omit it".into(),
            });
        }

        let workspace = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&txn)
            .await
            .internal()?
            .ok_or_else(|| YorishiroError::not_found("workspace not found"))?;

        crate::models::access::require_attributed_key_grant(
            &txn,
            workspace.tenant_id,
            user_id,
            scope,
        )
        .await?;

        if let Some(user_id) = user_id {
            let access = crate::models::access::workspace_access(
                &txn,
                workspace.tenant_id,
                workspace_id,
                user_id,
            )
            .await?
            .ok_or_else(|| YorishiroError::ScopeInsufficient {
                message: "this account has no access to the selected workspace".into(),
                hint: "grant workspace access before issuing a credential".into(),
            })?;
            if scope > access.max_scope {
                return Err(YorishiroError::ScopeInsufficient {
                    message: "requested scope exceeds this user's workspace authority".into(),
                    hint: "issue a lower-scoped key or change the workspace grant".into(),
                });
            }
        }

        if let Some(issuer) = issuer {
            let current = Model::current_context(&txn, issuer).await?;
            crate::models::access::require_key_grant(&txn, &current, scope, audit).await?;
            let issuer_key = Entity::find_by_id(issuer.api_key_id)
                .one(&txn)
                .await
                .internal()?
                .ok_or(YorishiroError::Unauthenticated)?;
            if let Some(limit) = issuer_key.expires_at
                && expires_at.is_none_or(|expiry| expiry > limit)
            {
                return Err(YorishiroError::ScopeInsufficient {
                    message: "requested expiry exceeds the issuing key's lifetime".into(),
                    hint: "set expires_at no later than the issuing key's expiry".into(),
                });
            }
        }

        let (prefix, plaintext) = generate_key();
        let key_hash = hash_key(&plaintext);

        let active = ActiveModel {
            workspace_id: ActiveValue::Set(Some(workspace_id)),
            tenant_id: ActiveValue::Set(workspace.tenant_id),
            user_id: ActiveValue::Set(user_id),
            key_hash: ActiveValue::Set(key_hash),
            key_prefix: ActiveValue::Set(prefix),
            name: ActiveValue::Set(name.to_owned()),
            expires_at: ActiveValue::Set(expires_at),
            scope: ActiveValue::Set(scope.as_db_str().to_string()),
            audit: ActiveValue::Set(audit),
            ..Default::default()
        };
        let inserted = active.insert(&txn).await.internal()?;
        let grant = inserted.grant_metadata(&txn).await?;
        txn.commit().await.internal()?;

        Ok(CreatedApiKey {
            id: inserted.id,
            workspace_id,
            scope,
            grant,
            key_prefix: inserted.key_prefix,
            name: inserted.name,
            created_at: inserted.created_at,
            plaintext,
        })
    }

    /// Every API key issued for a workspace, oldest first.
    /// Never returns `key_hash`: the plaintext key is shown once, at creation, and this listing exists for operators to see what exists and revoke by id, not to recover a lost key.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn list_for_workspace(
        conn: &impl ConnectionTrait,
        workspace_id: uuid::Uuid,
        page: crate::models::pagination::ListParams,
    ) -> Result<Vec<Model>, YorishiroError> {
        use crate::models::_entities::api_keys::Column;

        Entity::find()
            .filter(Column::WorkspaceId.eq(workspace_id))
            .order_by_asc(Column::CreatedAt)
            .limit(page.limit() as u64)
            .offset(page.offset() as u64)
            .all(conn)
            .await
            .internal()
    }

    /// Every key in a tenant, oldest first.
    pub(crate) async fn list_for_tenant(
        conn: &impl ConnectionTrait,
        tenant_id: uuid::Uuid,
    ) -> Result<Vec<Model>, YorishiroError> {
        use crate::models::_entities::api_keys::Column;

        Entity::find()
            .filter(Column::TenantId.eq(tenant_id))
            .order_by_asc(Column::CreatedAt)
            .order_by_asc(Column::Id)
            .all(conn)
            .await
            .internal()
    }

    /// Revokes an API key immediately while retaining its non-secret grant history.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn revoke(
        db: &sea_orm::DatabaseConnection,
        key_id: uuid::Uuid,
    ) -> Result<(), YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        let key = Self::find_by_id(key_id)
            .one(&txn)
            .await
            .internal()?
            .ok_or_else(|| {
                YorishiroError::not_found(format!("api key '{key_id}' was not found"))
            })?;
        crate::db::lock_for_update(&txn, &format!("tenant-membership:{}", key.tenant_id))
            .await
            .internal()?;
        Self::delete_for_tenant(&txn, key.tenant_id, key_id).await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    /// Revokes one of a tenant's keys without allowing an id from another tenant to match.
    ///
    /// # Errors
    /// Rejects stale administrator credentials, missing keys and persistence failures.
    pub async fn revoke_for_tenant(
        db: &sea_orm::DatabaseConnection,
        actor: &AuthContext,
        key_id: uuid::Uuid,
    ) -> Result<(), YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_current_tenant_admin(&txn, actor).await?;
        Self::delete_for_tenant(&txn, actor.tenant_id, key_id).await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn revoke_matching(
        conn: &impl ConnectionTrait,
        condition: sea_orm::Condition,
    ) -> Result<(), YorishiroError> {
        let keys = Entity::find()
            .filter(condition)
            .filter(crate::models::_entities::api_keys::Column::RevokedAt.is_null())
            .all(conn)
            .await
            .internal()?;
        for key in keys {
            Self::delete_for_tenant(conn, key.tenant_id, key.id).await?;
        }
        Ok(())
    }

    pub(crate) async fn rotate(
        db: &DatabaseConnection,
        actor: &AuthContext,
        key_id: Uuid,
    ) -> Result<CreatedApiKey, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_current_tenant_admin(&txn, actor).await?;
        let old = Entity::find_by_id(key_id)
            .one(&txn)
            .await
            .internal()?
            .filter(|key| key.tenant_id == actor.tenant_id)
            .ok_or_else(|| YorishiroError::not_found("api key not found"))?;
        if !old.is_active() {
            return Err(YorishiroError::Conflict {
                message: "only an active key can be rotated; issue a new key instead".into(),
            });
        }
        let issuer_binding = Entity::find_by_id(actor.api_key_id)
            .one(&txn)
            .await
            .internal()?
            .ok_or(YorishiroError::Unauthenticated)?;
        if issuer_binding.workspace_id.is_some() && old.workspace_id != issuer_binding.workspace_id
        {
            return Err(YorishiroError::ScopeInsufficient {
                message: "replacement target exceeds the issuing key's binding".into(),
                hint: "use a credential bound to the same workspace or an authorized tenant key"
                    .into(),
            });
        }
        let scope = stored_scope(&old.scope)?;
        let current = Model::current_context(&txn, actor).await?;
        crate::models::access::require_key_grant(&txn, &current, scope, old.audit).await?;
        if let Some(limit) = issuer_binding.expires_at
            && old.expires_at.is_none_or(|expiry| expiry > limit)
        {
            return Err(YorishiroError::ScopeInsufficient {
                message: "replacement expiry exceeds the issuing key's lifetime".into(),
                hint: "rotate using a credential with sufficient lifetime".into(),
            });
        }
        crate::models::access::require_attributed_key_grant(
            &txn,
            old.tenant_id,
            old.user_id,
            scope,
        )
        .await?;
        if let (Some(workspace), Some(user)) = (old.workspace_id, old.user_id) {
            let access =
                crate::models::access::workspace_access(&txn, old.tenant_id, workspace, user)
                    .await?
                    .ok_or(YorishiroError::Unauthenticated)?;
            if scope > access.max_scope {
                return Err(YorishiroError::ScopeInsufficient {
                    message: "key grant exceeds current workspace authority".into(),
                    hint: "issue a lower-scoped key instead".into(),
                });
            }
        }
        let (prefix, plaintext) = generate_key();
        let inserted = ActiveModel {
            tenant_id: ActiveValue::Set(old.tenant_id),
            workspace_id: ActiveValue::Set(old.workspace_id),
            user_id: ActiveValue::Set(old.user_id),
            name: ActiveValue::Set(old.name),
            scope: ActiveValue::Set(old.scope),
            audit: ActiveValue::Set(old.audit),
            expires_at: ActiveValue::Set(old.expires_at),
            key_prefix: ActiveValue::Set(prefix),
            key_hash: ActiveValue::Set(hash_key(&plaintext)),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .internal()?;
        Self::delete_for_tenant(&txn, actor.tenant_id, key_id).await?;
        Entity::update_many()
            .col_expr(
                crate::models::_entities::api_keys::Column::ReplacedById,
                sea_orm::sea_query::Expr::val(inserted.id),
            )
            .filter(crate::models::_entities::api_keys::Column::Id.eq(key_id))
            .exec(&txn)
            .await
            .internal()?;
        let grant = inserted.grant_metadata(&txn).await?;
        txn.commit().await.internal()?;
        Ok(CreatedApiKey {
            id: inserted.id,
            workspace_id: old.workspace_id.unwrap_or(actor.workspace_id),
            scope,
            grant,
            key_prefix: inserted.key_prefix,
            name: inserted.name,
            created_at: inserted.created_at,
            plaintext,
        })
    }

    async fn delete_for_tenant(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        key_id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::_entities::api_keys::Column;

        let result = Entity::update_many()
            .col_expr(
                Column::RevokedAt,
                sea_orm::sea_query::Expr::val(crate::time::now()),
            )
            .col_expr(
                Column::KeyHash,
                sea_orm::sea_query::Expr::val(hash_key(&generate_key().1)),
            )
            .filter(Column::Id.eq(key_id))
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::RevokedAt.is_null())
            .exec(conn)
            .await
            .internal()?;

        if result.rows_affected == 0 {
            Err(YorishiroError::not_found(format!(
                "api key '{key_id}' was not found"
            )))
        } else {
            Ok(())
        }
    }
}

/// A stored scope this crate does not define is a corrupt row, not a missing scope.
fn stored_scope(value: &str) -> Result<ApiKeyScope, YorishiroError> {
    ApiKeyScope::from_db_str(value).ok_or_else(|| {
        YorishiroError::Internal(anyhow::anyhow!(
            "unknown api key scope in database: {value}"
        ))
    })
}
