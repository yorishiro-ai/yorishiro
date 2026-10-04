//! Enterprise API-key persistence for keys bound to a tenant rather than one workspace.

use sea_orm::{ActiveValue, EntityTrait, PaginatorTrait};
use uuid::Uuid;

use crate::YorishiroError;
use crate::error::ResultExt;
use crate::models::_entities::{api_keys, tenant_tenants};
use crate::models::api_keys::{ApiKeyScope, hash_key};

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
            hint: "use one of: read, write, schema".into(),
        })?;

    let exists = tenant_tenants::Entity::find_by_id(tenant_id)
        .count(conn)
        .await
        .internal()?
        > 0;
    if !exists {
        return Err(YorishiroError::not_found(format!(
            "tenant '{tenant_id}' does not exist"
        )));
    }

    if let Some(user_id) = user_id {
        let role = crate::models::tenant_memberships::get_membership_role(conn, tenant_id, user_id)
            .await?
            .ok_or_else(|| {
                YorishiroError::not_found(format!(
                    "user '{user_id}' is not a member of tenant '{tenant_id}'"
                ))
            })?;
        if scope > role.max_scope() {
            return Err(YorishiroError::ScopeInsufficient {
                message: format!(
                    "this user's tenant role permits at most {:?} scope keys",
                    role.max_scope()
                ),
                hint: "issue a lower-scoped key, or raise the user's tenant role".into(),
            });
        }
    }

    // Same shape as base's own keys, so nothing downstream has to tell them apart by their text.
    // The randomness is two v4 UUIDs: 122 bits each, from the same CSPRNG base's own generator draws on, and `uuid` is already a dependency here.
    let prefix = format!("ysr_{}", &Uuid::new_v4().simple().to_string()[..12]);
    let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let plaintext = format!("{prefix}_{secret}");

    let active = api_keys::ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        workspace_id: ActiveValue::Set(None),
        key_hash: ActiveValue::Set(hash_key(&plaintext)),
        key_prefix: ActiveValue::Set(prefix),
        scope: ActiveValue::Set(scope.as_db_str().to_string()),
        user_id: ActiveValue::Set(user_id),
        ..Default::default()
    };
    let inserted = api_keys::Entity::insert(active)
        .exec_with_returning(conn)
        .await
        .internal()?;

    Ok(CreatedTenantApiKey {
        id: inserted.id,
        plaintext,
    })
}
