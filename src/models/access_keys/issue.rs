//! Issuing tenant keys, login keys and delegated keys.

use super::{
    ActiveModel, ActiveModelTrait, ActiveValue, AuthContext, CreatedAccessKey,
    CreatedTenantAccessKey, DatabaseConnection, Entity, EntityTrait, LOGIN_SESSION_HOURS, Model,
    PaginatorTrait, ResultExt, Uuid, YorishiroError, generate_key, hash_key, validate_key_name,
};

impl Entity {
    /// Issues an unnamed key for the tenant that owns `workspace_id`; the key itself is not bound to the workspace.
    ///
    /// # Errors
    /// Returns an error if the workspace does not exist or the key cannot be stored.
    pub async fn create_access_key(
        db: &DatabaseConnection,
        workspace_id: Uuid,
        user_id: Option<Uuid>,
    ) -> Result<CreatedTenantAccessKey, YorishiroError> {
        let tenant_id =
            crate::models::workspace_workspaces::get_workspace_tenant(db, workspace_id).await?;
        Self::create_tenant_access_key(db, tenant_id, user_id, "unnamed").await
    }

    /// Issues a key for a tenant, optionally attributed to one of its users.
    ///
    /// # Errors
    /// Returns an error if the tenant does not exist, the name is invalid or the user is not a member.
    pub async fn create_tenant_access_key(
        db: &DatabaseConnection,
        tenant_id: Uuid,
        user_id: Option<Uuid>,
        name: &str,
    ) -> Result<CreatedTenantAccessKey, YorishiroError> {
        use crate::models::_entities::tenant_tenants;

        let txn = crate::db::begin_write(db).await.internal()?;
        let name = validate_key_name(name)?;
        let exists = tenant_tenants::Entity::find_by_id(tenant_id)
            .count(&txn)
            .await
            .internal()?
            > 0;
        if !exists {
            return Err(YorishiroError::not_found(format!(
                "tenant '{tenant_id}' does not exist"
            )));
        }
        crate::models::iam::access::require_key_attribution(&txn, tenant_id, user_id).await?;
        let (prefix, plaintext) = generate_key();
        let inserted = ActiveModel {
            tenant_id: ActiveValue::Set(tenant_id),
            user_id: ActiveValue::Set(user_id),
            key_hash: ActiveValue::Set(hash_key(&plaintext)),
            key_prefix: ActiveValue::Set(prefix),
            name: ActiveValue::Set(name),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .internal()?;
        txn.commit().await.internal()?;
        Ok(CreatedTenantAccessKey {
            id: inserted.id,
            plaintext,
        })
    }

    pub(crate) async fn create_login_key(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        user_id: Uuid,
    ) -> Result<CreatedTenantAccessKey, YorishiroError> {
        crate::models::iam::access::require_key_attribution(txn, tenant_id, Some(user_id)).await?;
        let (prefix, plaintext) = super::generate_key_with_prefix(super::LOGIN_KEY_PREFIX);
        let inserted = ActiveModel {
            tenant_id: ActiveValue::Set(tenant_id),
            user_id: ActiveValue::Set(Some(user_id)),
            key_hash: ActiveValue::Set(hash_key(&plaintext)),
            key_prefix: ActiveValue::Set(prefix),
            name: ActiveValue::Set("login".to_string()),
            expires_at: ActiveValue::Set(Some(
                (crate::data::time::now() + chrono::Duration::hours(LOGIN_SESSION_HOURS)).into(),
            )),
            ..Default::default()
        }
        .insert(txn)
        .await
        .internal()?;
        Ok(CreatedTenantAccessKey {
            id: inserted.id,
            plaintext,
        })
    }

    /// Issues a key on the issuer's behalf, limited by the issuer's own expiry unless the issuer is a root session.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub(crate) async fn create_delegated_access_key(
        db: &DatabaseConnection,
        issuer: &AuthContext,
        name: &str,
        expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    ) -> Result<CreatedAccessKey, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        let name = validate_key_name(name)?;
        if expires_at.is_some_and(|expires| expires <= crate::data::time::now()) {
            return Err(YorishiroError::ValidationFailed {
                message: "key expiry must be in the future".into(),
                details: vec![],
                hint: "choose a future expires_at or omit it".into(),
            });
        }

        crate::models::iam::access::require_iam_action(
            &txn,
            issuer,
            crate::models::iam::policy::Action::CreateAccessKey,
            crate::models::iam::policy::Resource::Tenant {
                tenant_id: issuer.tenant_id,
            },
        )
        .await?;
        let current = Model::current_context(&txn, issuer).await?;
        crate::models::tenant_tenants::Model::require_active(&txn, current.tenant_id).await?;
        let issuer_key = Self::find_by_id(issuer.access_key_id)
            .one(&txn)
            .await
            .internal()?
            .ok_or(YorishiroError::Unauthenticated)?;
        if !Model::is_root_credential(&txn, issuer.access_key_id).await?
            && let Some(limit) = issuer_key.expires_at
            && expires_at.is_none_or(|expiry| expiry > limit)
        {
            return Err(YorishiroError::AccessDenied {
                message: "requested expiry exceeds the issuing key's lifetime".into(),
                hint: "set expires_at no later than the issuing key's expiry".into(),
            });
        }
        crate::models::iam::access::require_key_attribution(&txn, issuer.tenant_id, issuer.user_id)
            .await?;

        let (prefix, plaintext) = generate_key();
        let inserted = ActiveModel {
            tenant_id: ActiveValue::Set(issuer.tenant_id),
            user_id: ActiveValue::Set(issuer.user_id),
            key_hash: ActiveValue::Set(hash_key(&plaintext)),
            key_prefix: ActiveValue::Set(prefix),
            name: ActiveValue::Set(name),
            expires_at: ActiveValue::Set(expires_at),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .internal()?;
        let grant = inserted.grant_metadata();
        crate::models::iam_audit_events::record(
            &txn,
            issuer,
            crate::models::iam::policy::Action::CreateAccessKey,
            serde_json::json!({"access_key_id": inserted.id}),
        )
        .await?;
        txn.commit().await.internal()?;

        Ok(CreatedAccessKey {
            id: inserted.id,
            grant,
            key_prefix: inserted.key_prefix,
            name: inserted.name,
            created_at: inserted.created_at,
            plaintext,
        })
    }
}
