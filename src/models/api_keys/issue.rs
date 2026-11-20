//! Issuing keys: direct keys, login keys, role sessions and delegated keys.

use super::{
    ActiveModel, ActiveModelTrait, ActiveValue, ApiKeyScope, AuthContext, CreatedApiKey,
    CreatedTenantApiKey, DatabaseConnection, Entity, EntityTrait, LOGIN_SESSION_HOURS, Model,
    PaginatorTrait, ResultExt, Uuid, YorishiroError, generate_key, hash_key, validate_key_name,
};

impl Entity {
    /// Issues a new API key of the form `ysr_<prefix>_<secret>`, with 48 random alphanumeric secret characters (over 285 bits of entropy).
    /// SHA-256 is sufficient here rather than a slow KDF like bcrypt/argon2, since API keys already carry enough entropy that offline brute-forcing isn't a realistic threat.
    /// `audit` is independent of `scope`: it does not raise or lower where the key sits on the read/write/schema/migration ladder, only whether it additionally holds the separate grant `AuthContext::audit`'s doc comment describes.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn create_api_key(
        db: &DatabaseConnection,
        workspace_id: Uuid,
        scope: ApiKeyScope,
        user_id: Option<Uuid>,
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

    /// Issues a key without a workspace, so it acts on the caller's tenant and names its workspace per request.
    /// The role cap is the same one command applies: a key attributed to a user may not exceed what that user's tenant role permits, since the key can act as them.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn create_tenant_api_key(
        db: &DatabaseConnection,
        tenant_id: Uuid,
        scope: ApiKeyScope,
        user_id: Option<Uuid>,
        audit: bool,
        name: &str,
    ) -> Result<CreatedTenantApiKey, YorishiroError> {
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
            workspace_id: ActiveValue::Set(None),
            user_id: ActiveValue::Set(user_id),
            key_hash: ActiveValue::Set(hash_key(&plaintext)),
            key_prefix: ActiveValue::Set(prefix),
            name: ActiveValue::Set(name),
            scope: ActiveValue::Set(scope.as_db_str().to_string()),
            audit: ActiveValue::Set(audit),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .internal()?;
        txn.commit().await.internal()?;
        Ok(CreatedTenantApiKey {
            id: inserted.id,
            plaintext,
        })
    }

    pub(crate) async fn create_login_key(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        user_id: Uuid,
        scope: ApiKeyScope,
    ) -> Result<CreatedTenantApiKey, YorishiroError> {
        crate::models::iam::access::require_key_attribution(txn, tenant_id, Some(user_id)).await?;
        let (prefix, plaintext) = generate_key();
        let inserted = ActiveModel {
            tenant_id: ActiveValue::Set(tenant_id),
            workspace_id: ActiveValue::Set(None),
            user_id: ActiveValue::Set(Some(user_id)),
            key_hash: ActiveValue::Set(hash_key(&plaintext)),
            key_prefix: ActiveValue::Set(prefix),
            name: ActiveValue::Set("login".to_string()),
            scope: ActiveValue::Set(scope.as_db_str().to_string()),
            audit: ActiveValue::Set(false),
            expires_at: ActiveValue::Set(Some(
                (crate::data::time::now() + chrono::Duration::hours(LOGIN_SESSION_HOURS)).into(),
            )),
            ..Default::default()
        }
        .insert(txn)
        .await
        .internal()?;
        Ok(CreatedTenantApiKey {
            id: inserted.id,
            plaintext,
        })
    }

    pub(crate) async fn create_role_session(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        user_id: Option<Uuid>,
        role_id: Uuid,
        scope: ApiKeyScope,
        role_name: &str,
        expires_at: chrono::DateTime<chrono::FixedOffset>,
    ) -> Result<CreatedTenantApiKey, YorishiroError> {
        crate::models::iam::access::require_key_attribution(txn, tenant_id, user_id).await?;
        let (prefix, plaintext) = generate_key();
        let inserted = ActiveModel {
            tenant_id: ActiveValue::Set(tenant_id),
            workspace_id: ActiveValue::Set(None),
            user_id: ActiveValue::Set(user_id),
            role_id: ActiveValue::Set(Some(role_id)),
            key_hash: ActiveValue::Set(hash_key(&plaintext)),
            key_prefix: ActiveValue::Set(prefix),
            name: ActiveValue::Set(validate_key_name(&format!("role:{role_name}"))?),
            scope: ActiveValue::Set(scope.as_db_str().to_string()),
            audit: ActiveValue::Set(false),
            expires_at: ActiveValue::Set(Some(expires_at)),
            ..Default::default()
        }
        .insert(txn)
        .await
        .internal()?;
        Ok(CreatedTenantApiKey {
            id: inserted.id,
            plaintext,
        })
    }

    pub(crate) async fn create_delegated_api_key(
        db: &DatabaseConnection,
        issuer: &AuthContext,
        scope: ApiKeyScope,
        audit: bool,
        name: &str,
        expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    ) -> Result<CreatedApiKey, YorishiroError> {
        Self::create_named_api_key(
            db,
            issuer.require_workspace()?,
            scope,
            issuer.user_id,
            audit,
            name,
            Some(issuer),
            expires_at,
        )
        .await
    }

    /// Issues a tenant-scoped key on the issuer's behalf: no workspace binding, per-request workspace selection, tenant Admin or Owner only.
    /// A workspace-bound issuer cannot mint one, since its authority never left that workspace.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub(crate) async fn create_delegated_tenant_api_key(
        db: &DatabaseConnection,
        issuer: &AuthContext,
        scope: ApiKeyScope,
        audit: bool,
        name: &str,
        expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    ) -> Result<CreatedApiKey, YorishiroError> {
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
            ApiKeyScope::Read,
            crate::models::iam::policy::Action::CredentialCreate,
            crate::models::iam::policy::Resource::Tenant {
                tenant_id: issuer.tenant_id,
            },
        )
        .await?;
        let current = Model::current_context(&txn, issuer).await?;
        crate::models::iam::access::require_key_grant(&txn, &current, scope, audit).await?;
        let issuer_key = Self::find_by_id(issuer.api_key_id)
            .one(&txn)
            .await
            .internal()?
            .ok_or(YorishiroError::Unauthenticated)?;
        if issuer_key.workspace_id.is_some()
            && !Model::is_root_credential(&txn, issuer.api_key_id).await?
        {
            return Err(YorishiroError::ScopeInsufficient {
                message: "a tenant-scoped key can only be issued with a tenant-scoped credential"
                    .into(),
                hint: "issue it from a key without a workspace binding".into(),
            });
        }
        if !Model::is_root_credential(&txn, issuer.api_key_id).await?
            && let Some(limit) = issuer_key.expires_at
            && expires_at.is_none_or(|expiry| expiry > limit)
        {
            return Err(YorishiroError::ScopeInsufficient {
                message: "requested expiry exceeds the issuing key's lifetime".into(),
                hint: "set expires_at no later than the issuing key's expiry".into(),
            });
        }
        crate::models::iam::access::require_key_attribution(&txn, issuer.tenant_id, issuer.user_id)
            .await?;

        let (prefix, plaintext) = generate_key();
        let inserted = ActiveModel {
            tenant_id: ActiveValue::Set(issuer.tenant_id),
            workspace_id: ActiveValue::Set(None),
            user_id: ActiveValue::Set(issuer.user_id),
            key_hash: ActiveValue::Set(hash_key(&plaintext)),
            key_prefix: ActiveValue::Set(prefix),
            name: ActiveValue::Set(name),
            scope: ActiveValue::Set(scope.as_db_str().to_string()),
            audit: ActiveValue::Set(audit),
            expires_at: ActiveValue::Set(expires_at),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .internal()?;
        let grant = inserted.grant_metadata(&txn).await?;
        txn.commit().await.internal()?;

        Ok(CreatedApiKey {
            id: inserted.id,
            workspace_id: None,
            scope,
            grant,
            key_prefix: inserted.key_prefix,
            name: inserted.name,
            created_at: inserted.created_at,
            plaintext,
        })
    }

    /// Issues a key and validates attribution while holding the membership fence.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the table columns written in one statement"
    )]
    async fn create_named_api_key(
        db: &DatabaseConnection,
        workspace_id: Uuid,
        scope: ApiKeyScope,
        user_id: Option<Uuid>,
        audit: bool,
        name: &str,
        issuer: Option<&AuthContext>,
        expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    ) -> Result<CreatedApiKey, YorishiroError> {
        use crate::models::_entities::workspace_workspaces;

        let txn = crate::db::begin_write(db).await.internal()?;
        let name = validate_key_name(name)?;
        if expires_at.is_some_and(|expires| expires <= crate::data::time::now()) {
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

        crate::models::iam::access::require_key_attribution(&txn, workspace.tenant_id, user_id)
            .await?;

        if let Some(issuer) = issuer {
            crate::models::iam::access::require_iam_action(
                &txn,
                issuer,
                ApiKeyScope::Read,
                crate::models::iam::policy::Action::CredentialCreate,
                crate::models::iam::policy::Resource::Tenant {
                    tenant_id: workspace.tenant_id,
                },
            )
            .await?;
            let current = Model::current_context(&txn, issuer).await?;
            crate::models::iam::access::require_key_grant(&txn, &current, scope, audit).await?;
            let issuer_key = Self::find_by_id(issuer.api_key_id)
                .one(&txn)
                .await
                .internal()?
                .ok_or(YorishiroError::Unauthenticated)?;
            if !Model::is_root_credential(&txn, issuer.api_key_id).await?
                && let Some(limit) = issuer_key.expires_at
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
            name: ActiveValue::Set(name.clone()),
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
            workspace_id: Some(workspace_id),
            scope,
            grant,
            key_prefix: inserted.key_prefix,
            name: inserted.name,
            created_at: inserted.created_at,
            plaintext,
        })
    }
}
