//! Enterprise access-key persistence for keys that act on a tenant.

use uuid::Uuid;

use crate::YorishiroError;

/// A freshly issued tenant-scoped key.
pub use crate::models::access_keys::CreatedTenantAccessKey;

/// Issues a tenant-scoped key through the base persistence path.
///
/// **`conn` must be the identity pool (`DbHandle::identity`, wrapped as a `sea_orm::DatabaseConnection`), not the tenant pool.** The insert path reads `tenant_tenants`, which is not granted to `yorishiro_app` (the tenant pool's role).
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn create_tenant_access_key(
    conn: &sea_orm::DatabaseConnection,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<CreatedTenantAccessKey, YorishiroError> {
    crate::models::access_keys::Entity::create_tenant_access_key(
        conn, tenant_id, user_id, "unnamed",
    )
    .await
}
