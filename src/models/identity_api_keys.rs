use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, TransactionTrait};

use crate::error::{ResultExt, YorishiroError};
use crate::services::auth::{
    ApiKeyScope, CreatedApiKey, KEY_PREFIX_BYTES, KEY_SECRET_BYTES, hash_key, random_hex,
};

pub use super::_entities::identity_api_keys::{ActiveModel, Entity, Model};
pub type IdentityApiKeys = Entity;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        Ok(self)
    }
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {
    /// Issues a new API key of the form `ysr_<prefix>_<secret>`, where only the `secret` part
    /// (192 bits) is the actual credential. SHA-256 is sufficient here rather than a slow KDF
    /// like bcrypt/argon2, since API keys already carry enough entropy that offline
    /// brute-forcing isn't a realistic threat.
    pub async fn create_api_key(
        db: &sea_orm::DatabaseConnection,
        workspace_id: uuid::Uuid,
        scope: ApiKeyScope,
        user_id: Option<uuid::Uuid>,
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
            ..Default::default()
        };
        let inserted = active.insert(&txn).await.internal()?;
        txn.commit().await.internal()?;

        Ok(CreatedApiKey {
            id: inserted.id,
            workspace_id,
            scope,
            user_id,
            plaintext,
        })
    }
}
