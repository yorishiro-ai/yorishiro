use rand::Rng;
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, QueryOrder, QuerySelect, TransactionTrait};
use sha2::{Digest, Sha256};

use crate::db::DbHandle;
use crate::db_enum::db_enum;
use crate::error::{ResultExt, YorishiroError};

const KEY_PREFIX_BYTES: usize = 6;
const KEY_SECRET_BYTES: usize = 24;

db_enum! {
    /// Permission level held by an API key.
    /// Declaration order feeds the derived `Ord`: `Read < Write < Schema < Migration`, a higher scope subsumes lower ones.
    /// The wire form matches the DB `scope` column ('read'/'write'/'schema'/'migration').
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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

pub struct CreatedApiKey {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub scope: ApiKeyScope,
    /// The raw API key string.
    /// Only its hash is stored in the DB, so this return value is the only place it can ever be obtained.
    pub plaintext: String,
}

/// Lowercase-hex-encodes `byte_len` random bytes.
pub(crate) fn random_hex(byte_len: usize) -> String {
    let mut bytes = vec![0u8; byte_len];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Hashes a presented key into the form stored in `api_keys.key_hash`.
pub fn hash_key(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}

pub use crate::models::_entities::api_keys::{ActiveModel, Entity, Model};
pub type IdentityApiKeys = Entity;

#[async_trait::async_trait]
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

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {
    /// Verifies a presented raw API key and resolves the workspace, tenant, and scope it belongs to.
    ///
    /// At this point neither the workspace nor the tenant is known yet, so RLS's `app.current_workspace`/`app.current_tenant` can't be set, which is why this takes the whole [`DbHandle`] rather than a scoped connection.
    ///
    /// Raw SQL: `authenticate_api_key` is a SECURITY DEFINER function, so this bypasses RLS on api_keys/workspace_workspaces, and limits the columns it returns to id/workspace_id/tenant_id/scope/user_id/audit (never key_hash itself).
    pub async fn authenticate(
        db: &DbHandle,
        presented_key: &str,
    ) -> Result<AuthContext, YorishiroError> {
        let row: Option<(uuid::Uuid, uuid::Uuid, uuid::Uuid, String, Option<uuid::Uuid>, bool)> =
            sqlx::query_as(
                "SELECT id, workspace_id, tenant_id, scope, user_id, audit FROM authenticate_api_key($1)",
            )
            .bind(hash_key(presented_key))
            .fetch_optional(db.tenant.pool())
            .await
            .internal()?;

        let (api_key_id, workspace_id, tenant_id, scope, user_id, audit) =
            row.ok_or(YorishiroError::Unauthenticated)?;
        Ok(AuthContext {
            api_key_id,
            workspace_id,
            tenant_id,
            scope: stored_scope(&scope)?,
            user_id,
            audit,
        })
    }

    /// SQLite equivalent of [`Self::authenticate`], for a deployment with no `DbHandle` (see `Hooks::after_context`).
    ///
    /// The Postgres path goes through the SECURITY DEFINER function specifically to read rows RLS would otherwise hide from an unauthenticated caller; SQLite has no RLS at all, so there is nothing to bypass, and this queries the tables directly with the SeaORM entity API.
    /// Matches the SQL function's single-argument overload exactly: only a workspace-scoped key (`workspace_id` set) resolves; a tenant-scoped key matches nothing here either, same as on Postgres.
    ///
    /// Deliberately not routed through the `Authenticator` trait: that trait exists so `ee/` can swap the authentication rule, and `ee/` does not run against SQLite, so there is no second implementation for the trait to replace on this backend.
    pub async fn authenticate_sqlite(
        conn: &impl ConnectionTrait,
        presented_key: &str,
    ) -> Result<AuthContext, YorishiroError> {
        let (key, tenant_id) = Self::find_sqlite_auth_context(conn, hash_key(presented_key))
            .await?
            .ok_or(YorishiroError::Unauthenticated)?;
        let workspace_id = key.workspace_id.ok_or(YorishiroError::Unauthenticated)?;
        Ok(AuthContext {
            api_key_id: key.id,
            workspace_id,
            tenant_id,
            scope: stored_scope(&key.scope)?,
            user_id: key.user_id,
            audit: key.audit,
        })
    }

    /// Records the key's last-used timestamp on a raw `sqlx` connection.
    /// Best-effort: doesn't affect authentication outcomes, so callers don't need to fail the whole request if it errors.
    ///
    /// Deliberately not run on the request's `DatabaseTransaction`: a read-only handler drops that transaction without committing, which would silently roll this update back along with it.
    /// Every caller uses a short-lived connection from `TenantDb::acquire_for_workspace` instead.
    pub(crate) async fn touch_last_used_on_connection(
        conn: &mut sqlx::PgConnection,
        api_key_id: uuid::Uuid,
    ) -> Result<(), YorishiroError> {
        sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1")
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
                sea_orm::sea_query::Expr::value(chrono::Utc::now()),
            )
            .filter(Column::Id.eq(api_key_id))
            .exec(conn)
            .await
        {
            tracing::warn!(error = %err, "failed to update api key last_used_at");
        }
    }

    /// Issues a new API key of the form `ysr_<prefix>_<secret>`, where only the `secret` part (192 bits) is the actual credential.
    /// SHA-256 is sufficient here rather than a slow KDF like bcrypt/argon2, since API keys already carry enough entropy that offline brute-forcing isn't a realistic threat.
    /// `audit` is independent of `scope`: it does not raise or lower where the key sits on the read/write/schema/migration ladder, only whether it additionally holds the separate grant `AuthContext::audit`'s doc comment describes.
    /// Every caller except the `create_api_key` CLI task passes `false`: an audit-reading key is an explicit operator decision, never a side effect of signup, login, or OAuth provisioning a key for an ordinary user.
    pub async fn create_api_key(
        db: &sea_orm::DatabaseConnection,
        workspace_id: uuid::Uuid,
        scope: ApiKeyScope,
        user_id: Option<uuid::Uuid>,
        audit: bool,
    ) -> Result<CreatedApiKey, YorishiroError> {
        Self::create_named_api_key(db, workspace_id, scope, user_id, audit, "unnamed").await
    }

    /// Issues a key with a caller-visible name.
    pub(crate) async fn create_named_api_key(
        db: &sea_orm::DatabaseConnection,
        workspace_id: uuid::Uuid,
        scope: ApiKeyScope,
        user_id: Option<uuid::Uuid>,
        audit: bool,
        name: &str,
    ) -> Result<CreatedApiKey, YorishiroError> {
        use crate::models::_entities::workspace_workspaces;

        let txn = db.begin().await.internal()?;

        let workspace = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&txn)
            .await
            .internal()?
            .ok_or_else(|| YorishiroError::not_found("workspace not found"))?;

        let prefix = format!("ysr_{}", random_hex(KEY_PREFIX_BYTES));
        let secret = random_hex(KEY_SECRET_BYTES);
        let plaintext = format!("{prefix}_{secret}");
        let key_hash = hash_key(&plaintext);

        let active = ActiveModel {
            workspace_id: ActiveValue::Set(Some(workspace_id)),
            tenant_id: ActiveValue::Set(workspace.tenant_id),
            user_id: ActiveValue::Set(user_id),
            key_hash: ActiveValue::Set(key_hash),
            key_prefix: ActiveValue::Set(prefix),
            name: ActiveValue::Set(name.to_owned()),
            scope: ActiveValue::Set(scope.as_db_str().to_string()),
            audit: ActiveValue::Set(audit),
            ..Default::default()
        };
        let inserted = active.insert(&txn).await.internal()?;
        txn.commit().await.internal()?;

        Ok(CreatedApiKey {
            id: inserted.id,
            workspace_id,
            scope,
            plaintext,
        })
    }

    /// Every API key issued for a workspace, oldest first.
    /// Never returns `key_hash`: the plaintext key is shown once, at creation, and this listing exists for operators to see what exists and revoke by id, not to recover a lost key.
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

    /// Deletes an API key by id, revoking it immediately: authentication looks up the key on every request, so there is no cached credential to also invalidate.
    pub async fn revoke(
        conn: &impl ConnectionTrait,
        key_id: uuid::Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::_entities::api_keys::Column;

        let result = Entity::delete_many()
            .filter(Column::Id.eq(key_id))
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

    /// Revokes one of a tenant's keys without allowing an id from another tenant to match.
    pub(crate) async fn revoke_for_tenant(
        conn: &impl ConnectionTrait,
        tenant_id: uuid::Uuid,
        key_id: uuid::Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::_entities::api_keys::Column;

        let result = Entity::delete_many()
            .filter(Column::Id.eq(key_id))
            .filter(Column::TenantId.eq(tenant_id))
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
