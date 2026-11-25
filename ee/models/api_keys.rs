//! Enterprise API-key persistence for keys bound to a tenant rather than one workspace.

use uuid::Uuid;

use crate::YorishiroError;

/// A freshly issued tenant-scoped key.
pub use crate::models::api_keys::CreatedTenantApiKey;

/// Issues a tenant-scoped key through the base persistence path.
///
/// **`conn` must be the identity pool (`DbHandle::identity`, wrapped as a `sea_orm::DatabaseConnection`), not the tenant pool.** The insert path reads `tenant_tenants`, which is not granted to `yorishiro_app` (the tenant pool's role).
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn create_tenant_api_key(
    conn: &sea_orm::DatabaseConnection,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<CreatedTenantApiKey, YorishiroError> {
    crate::models::api_keys::Entity::create_tenant_api_key(
        conn, tenant_id, user_id, false, "unnamed",
    )
    .await
}
