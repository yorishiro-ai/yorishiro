use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DisableTenantRequest {
    /// Explicitly confirms the authenticated owner's tenant UUID.
    pub confirmed_tenant_id: Uuid,
}
