use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, QueryOrder, QuerySelect, TransactionTrait};

use crate::error::{ResultExt, YorishiroError};
use crate::services::auth::{
    ApiKeyScope, CreatedApiKey, KEY_PREFIX_BYTES, KEY_SECRET_BYTES, hash_key, random_hex,
};

pub use super::_entities::identity_api_keys::{ActiveModel, Entity, Model};
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
    /// Issues a new API key of the form `ysr_<prefix>_<secret>`, where only the `secret` part (192 bits) is the actual credential.
    /// SHA-256 is sufficient here rather than a slow KDF like bcrypt/argon2, since API keys already carry enough entropy that offline brute-forcing isn't a realistic threat.
    /// `audit` is independent of `scope`: it does not raise or lower where the key sits on the read/write/schema/migration ladder, only whether it additionally holds the separate grant `services::auth::AuthContext::audit`'s doc comment describes.
    /// Every caller except the `create_api_key` CLI task passes `false`: an audit-reading key is an explicit operator decision, never a side effect of signup, login, or OAuth provisioning a key for an ordinary user.
    pub async fn create_api_key(
        db: &sea_orm::DatabaseConnection,
        workspace_id: uuid::Uuid,
        scope: ApiKeyScope,
        user_id: Option<uuid::Uuid>,
        audit: bool,
    ) -> Result<CreatedApiKey, YorishiroError> {
        use super::_entities::identity_workspaces;

        let txn = db.begin().await.internal()?;

        let workspace = identity_workspaces::Entity::find_by_id(workspace_id)
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
            user_id,
            audit,
            plaintext,
        })
    }

    /// Every API key issued for a workspace, oldest first.
    /// Never returns `key_hash`: the plaintext key is shown once, at creation, and this listing exists for operators to see what exists and revoke by id, not to recover a lost key.
    pub async fn list_for_workspace(
        conn: &impl ConnectionTrait,
        workspace_id: uuid::Uuid,
        page: super::pagination::ListParams,
    ) -> Result<Vec<Model>, YorishiroError> {
        use super::_entities::identity_api_keys::Column;

        Entity::find()
            .filter(Column::WorkspaceId.eq(workspace_id))
            .order_by_asc(Column::CreatedAt)
            .limit(page.limit() as u64)
            .offset(page.offset() as u64)
            .all(conn)
            .await
            .internal()
    }

    /// Deletes an API key by id, revoking it immediately: authentication looks up the key on every request, so there is no cached credential to also invalidate.
    pub async fn revoke(
        conn: &impl ConnectionTrait,
        key_id: uuid::Uuid,
    ) -> Result<(), YorishiroError> {
        use super::_entities::identity_api_keys::Column;

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
}
