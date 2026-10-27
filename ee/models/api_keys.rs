//! Enterprise API-key persistence for keys bound to a tenant rather than one workspace.

use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, PaginatorTrait};
use uuid::Uuid;

use crate::YorishiroError;
use crate::error::ResultExt;
use crate::models::_entities::{api_keys, tenant_tenants};
use crate::models::api_keys::{ApiKeyScope, generate_key, hash_key};

/// A freshly issued tenant-scoped key.
/// The plaintext exists only here: only its hash is stored.
pub struct CreatedTenantApiKey {
    pub id: Uuid,
    pub plaintext: String,
}

/// Issues a tenant-scoped key.
///
/// Base's own `create_api_key` always records a workspace, so a key with none cannot be made through it: this writes the row directly.
/// The role cap is the same one that command applies: a key attributed to a user may not exceed what that user's tenant role permits, since the key can act as them.
///
/// **`conn` must be the identity pool (`DbHandle::identity`, wrapped as a `sea_orm::DatabaseConnection`), not the tenant pool.** This reads `tenant_tenants` and `tenant_memberships`, and neither is granted to `yorishiro_app` (the tenant pool's role): calling this against the tenant pool fails with "permission denied for table tenant_tenants".
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn create_tenant_api_key(
    conn: &sea_orm::DatabaseConnection,
    tenant_id: Uuid,
    scope: &str,
    user_id: Option<Uuid>,
) -> Result<CreatedTenantApiKey, YorishiroError> {
    let scope =
        ApiKeyScope::from_db_str(scope).ok_or_else(|| YorishiroError::ValidationFailed {
            message: format!("unknown scope '{scope}'"),
            details: Vec::new(),
            hint: "use one of: read, write, schema, migration".into(),
        })?;

    let txn = crate::db::begin_write(conn).await.internal()?;

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

    crate::models::access::require_attributed_key_grant(&txn, tenant_id, user_id, scope).await?;

    let (prefix, plaintext) = generate_key();

    let active = api_keys::ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        workspace_id: ActiveValue::Set(None),
        key_hash: ActiveValue::Set(hash_key(&plaintext)),
        key_prefix: ActiveValue::Set(prefix),
        scope: ActiveValue::Set(scope.as_db_str().to_string()),
        user_id: ActiveValue::Set(user_id),
        ..Default::default()
    };
    let inserted = active.insert(&txn).await.internal()?;
    txn.commit().await.internal()?;

    Ok(CreatedTenantApiKey {
        id: inserted.id,
        plaintext,
    })
}
