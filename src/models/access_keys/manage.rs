//! Listing, updating and deleting access keys.

use super::{
    AccessKeyChange, ActiveModel, ActiveModelTrait, ActiveValue, AuthContext, ColumnTrait,
    ConnectionTrait, DatabaseConnection, Entity, EntityTrait, Expr, KeyGrant, Model, QueryFilter,
    QueryOrder, ResultExt, Uuid, YorishiroError, generate_key, hash_key, validate_key_name,
};

impl Entity {
    /// Every key in a tenant, oldest first.
    pub(crate) async fn list_with_grants_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
    ) -> Result<Vec<(Model, KeyGrant)>, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::iam::policy::Action::ListAccessKeys,
            crate::models::iam::policy::Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let mut records = Vec::new();
        for key in Self::list_for_tenant(&txn, actor.tenant_id).await? {
            let grant = key.grant_metadata();
            records.push((key, grant));
        }
        Ok(records)
    }

    pub(crate) async fn list_for_tenant(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
    ) -> Result<Vec<Model>, YorishiroError> {
        use crate::models::_entities::access_keys::Column;

        Self::find()
            .filter(Column::TenantId.eq(tenant_id))
            .order_by_asc(Column::CreatedAt)
            .order_by_asc(Column::Id)
            .all(conn)
            .await
            .internal()
    }

    /// Revokes an access key immediately while retaining its non-secret grant history.
    ///
    /// # Errors
    /// Returns an error if the key does not exist or the revocation cannot be stored.
    pub async fn revoke(
        db: &DatabaseConnection,
        access_key_id: Uuid,
    ) -> Result<(), YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        let key = Self::find_by_id(access_key_id)
            .one(&txn)
            .await
            .internal()?
            .ok_or_else(|| {
                YorishiroError::not_found(format!("access key '{access_key_id}' was not found"))
            })?;
        crate::db::lock_for_update(&txn, &format!("tenant-membership:{}", key.tenant_id))
            .await
            .internal()?;
        Self::delete_for_tenant(&txn, key.tenant_id, access_key_id).await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    /// Deletes one of a tenant's keys without allowing an id from another tenant to match.
    ///
    /// # Errors
    /// Rejects stale administrator credentials, missing keys and persistence failures.
    pub async fn delete_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
        access_key_id: Uuid,
    ) -> Result<(), YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::iam::policy::Action::DeleteAccessKey,
            crate::models::iam::policy::Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        Self::management_target(&txn, actor, access_key_id).await?;
        Self::delete_for_tenant(&txn, actor.tenant_id, access_key_id).await?;
        crate::models::api_events::record(
            &txn,
            actor,
            crate::models::iam::policy::Action::DeleteAccessKey,
            serde_json::json!({"access_key_id": access_key_id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    /// Changes an access key's name, expiry or enabled flag within the actor's tenant.
    ///
    /// A caller that is not a root sign-in session cannot extend a key beyond its own expiry.
    ///
    /// # Errors
    /// Rejects stale credentials, a missing or revoked key, an invalid name or expiry, and persistence failures.
    pub(crate) async fn update_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
        access_key_id: Uuid,
        change: AccessKeyChange,
    ) -> Result<(Model, KeyGrant), YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::iam::policy::Action::UpdateAccessKey,
            crate::models::iam::policy::Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let key = Self::management_target(&txn, actor, access_key_id).await?;
        let mut active: ActiveModel = key.into();
        if let Some(name) = change.name {
            active.name = ActiveValue::Set(validate_key_name(&name)?);
        }
        if let Some(enabled) = change.enabled {
            active.enabled = ActiveValue::Set(enabled);
        }
        if change.replace_expires_at {
            let expires_at = change.expires_at;
            if expires_at.is_some_and(|expiry| expiry <= crate::data::time::now()) {
                return Err(YorishiroError::ValidationFailed {
                    message: "key expiry must be in the future".into(),
                    details: vec![],
                    hint: "choose a future expires_at or clear it".into(),
                });
            }
            let caller = Self::find_by_id(actor.access_key_id)
                .one(&txn)
                .await
                .internal()?
                .ok_or(YorishiroError::Unauthenticated)?;
            if !Model::is_root_credential(&txn, actor.access_key_id).await?
                && let Some(limit) = caller.expires_at
                && expires_at.is_none_or(|expiry| expiry > limit)
            {
                return Err(YorishiroError::AccessDenied {
                    message: "requested expiry exceeds the calling key's lifetime".into(),
                    hint: "set expires_at no later than the calling key's expiry".into(),
                });
            }
            active.expires_at = ActiveValue::Set(expires_at);
        }
        let updated = active.update(&txn).await.map_err(|error| {
            YorishiroError::not_found_on_missing(error, "access key was not found")
        })?;
        let grant = updated.grant_metadata();
        crate::models::api_events::record(
            &txn,
            actor,
            crate::models::iam::policy::Action::UpdateAccessKey,
            serde_json::json!({"access_key_id": access_key_id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok((updated, grant))
    }

    async fn management_target(
        txn: &sea_orm::DatabaseTransaction,
        actor: &AuthContext,
        access_key_id: Uuid,
    ) -> Result<Model, YorishiroError> {
        use crate::models::_entities::access_keys::Column;

        let key = Self::find_by_id(access_key_id)
            .filter(Column::TenantId.eq(actor.tenant_id))
            .filter(Column::RevokedAt.is_null())
            .one(txn)
            .await
            .internal()?
            .ok_or_else(|| {
                YorishiroError::not_found(format!("access key '{access_key_id}' was not found"))
            })?;
        if key.has_root_principal(txn).await?
            && !Model::is_root_credential(txn, actor.access_key_id).await?
        {
            return Err(YorishiroError::AccessDenied {
                message: "only a root sign-in session can manage root credentials".into(),
                hint: "sign in as the tenant root".into(),
            });
        }
        Ok(key)
    }

    pub(crate) async fn revoke_matching(
        conn: &impl ConnectionTrait,
        condition: sea_orm::Condition,
    ) -> Result<(), YorishiroError> {
        let keys = Self::find()
            .filter(condition)
            .filter(crate::models::_entities::access_keys::Column::RevokedAt.is_null())
            .all(conn)
            .await
            .internal()?;
        for key in keys {
            Self::delete_for_tenant(conn, key.tenant_id, key.id).await?;
        }
        Ok(())
    }

    async fn delete_for_tenant(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        access_key_id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::_entities::access_keys::Column;

        let result = Self::update_many()
            .col_expr(Column::RevokedAt, Expr::val(crate::data::time::now()))
            .col_expr(Column::KeyHash, Expr::val(hash_key(&generate_key().1)))
            .filter(Column::Id.eq(access_key_id))
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::RevokedAt.is_null())
            .exec(conn)
            .await
            .internal()?;

        if result.rows_affected == 0 {
            Err(YorishiroError::not_found(format!(
                "access key '{access_key_id}' was not found"
            )))
        } else {
            Ok(())
        }
    }
}
