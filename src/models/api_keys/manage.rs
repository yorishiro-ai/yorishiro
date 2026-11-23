//! Listing, revoking and rotating keys.

use super::{
    ActiveModel, ActiveModelTrait, ActiveValue, ApiKeyScope, AuthContext, ColumnTrait,
    ConnectionTrait, CreatedApiKey, DatabaseConnection, Entity, EntityTrait, Expr, KeyGrant, Model,
    QueryFilter, QueryOrder, QuerySelect, ResultExt, Uuid, YorishiroError, generate_key, hash_key,
    stored_scope,
};

impl Entity {
    /// Every API key issued for a workspace, oldest first.
    /// Never returns `key_hash`: the plaintext key is shown once, at creation, and this listing exists for operators to see what exists and revoke by id, not to recover a lost key.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn list_for_workspace(
        conn: &impl ConnectionTrait,
        workspace_id: Uuid,
        page: crate::models::pagination::ListParams,
    ) -> Result<Vec<Model>, YorishiroError> {
        use crate::models::_entities::api_keys::Column;

        Self::find()
            .filter(Column::WorkspaceId.eq(workspace_id))
            .order_by_asc(Column::CreatedAt)
            .limit(page.limit())
            .offset(page.offset())
            .all(conn)
            .await
            .internal()
    }

    /// Every key in a tenant, oldest first.
    pub(crate) async fn list_with_grants_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
    ) -> Result<Vec<(Model, KeyGrant)>, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            ApiKeyScope::Read,
            crate::models::iam::policy::Action::CredentialList,
            crate::models::iam::policy::Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let mut records = Vec::new();
        for key in Self::list_for_tenant(&txn, actor.tenant_id).await? {
            let grant = key.grant_metadata(&txn).await?;
            records.push((key, grant));
        }
        Ok(records)
    }

    pub(crate) async fn list_for_tenant(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
    ) -> Result<Vec<Model>, YorishiroError> {
        use crate::models::_entities::api_keys::Column;

        Self::find()
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
    pub async fn revoke(db: &DatabaseConnection, key_id: Uuid) -> Result<(), YorishiroError> {
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
        db: &DatabaseConnection,
        actor: &AuthContext,
        key_id: Uuid,
    ) -> Result<(), YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            ApiKeyScope::Read,
            crate::models::iam::policy::Action::CredentialRevoke,
            crate::models::iam::policy::Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        Self::delete_for_tenant(&txn, actor.tenant_id, key_id).await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn revoke_matching(
        conn: &impl ConnectionTrait,
        condition: sea_orm::Condition,
    ) -> Result<(), YorishiroError> {
        let keys = Self::find()
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

    #[expect(
        clippy::too_many_lines,
        reason = "validation, revocation and replacement share one transaction"
    )]
    pub(crate) async fn rotate(
        db: &DatabaseConnection,
        actor: &AuthContext,
        key_id: Uuid,
    ) -> Result<CreatedApiKey, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            ApiKeyScope::Read,
            crate::models::iam::policy::Action::CredentialRotate,
            crate::models::iam::policy::Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let old = Self::find_by_id(key_id)
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
        if old.is_login_session() {
            return Err(YorishiroError::Conflict {
                message: "login sessions cannot be rotated; sign in again".into(),
            });
        }
        let root_session = Model::is_root_credential(&txn, actor.api_key_id).await?;
        if !root_session && old.user_id != actor.user_id {
            return Err(YorishiroError::ScopeInsufficient {
                message: "only a root sign-in session can rotate another principal's key".into(),
                hint: "rotate your own key or ask the tenant root".into(),
            });
        }
        let issuer_binding = Self::find_by_id(actor.api_key_id)
            .one(&txn)
            .await
            .internal()?
            .ok_or(YorishiroError::Unauthenticated)?;
        if issuer_binding.workspace_id.is_some()
            && old.workspace_id != issuer_binding.workspace_id
            && !root_session
        {
            return Err(YorishiroError::ScopeInsufficient {
                message: "replacement target exceeds the issuing key's binding".into(),
                hint: "use a credential bound to the same workspace or an authorized tenant key"
                    .into(),
            });
        }
        let scope = stored_scope(&old.scope)?;
        let current = Model::current_context(&txn, actor).await?;
        crate::models::iam::access::require_key_grant(&txn, &current, scope, old.audit).await?;
        if !root_session
            && let Some(limit) = issuer_binding.expires_at
            && old.expires_at.is_none_or(|expiry| expiry > limit)
        {
            return Err(YorishiroError::ScopeInsufficient {
                message: "replacement expiry exceeds the issuing key's lifetime".into(),
                hint: "rotate using a credential with sufficient lifetime".into(),
            });
        }
        crate::models::iam::access::require_key_attribution(&txn, old.tenant_id, old.user_id)
            .await?;
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
        Self::update_many()
            .col_expr(
                crate::models::_entities::api_keys::Column::ReplacedById,
                Expr::val(inserted.id),
            )
            .filter(crate::models::_entities::api_keys::Column::Id.eq(key_id))
            .exec(&txn)
            .await
            .internal()?;
        let grant = inserted.grant_metadata(&txn).await?;
        txn.commit().await.internal()?;
        Ok(CreatedApiKey {
            id: inserted.id,
            workspace_id: old.workspace_id,
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

        let result = Self::update_many()
            .col_expr(Column::RevokedAt, Expr::val(crate::data::time::now()))
            .col_expr(Column::KeyHash, Expr::val(hash_key(&generate_key().1)))
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
