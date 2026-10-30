use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DisableTenantRequest {
    /// Explicitly confirms the authenticated owner's tenant UUID.
    pub confirmed_tenant_id: Uuid,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateTenantRequest {
    pub name: Option<String>,
    /// Explicitly replace the manual workspace cap, including clearing it with null.
    #[serde(default)]
    pub replace_max_workspaces: bool,
    pub max_workspaces: Option<i32>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TenantRecord {
    pub id: Uuid,
    pub name: String,
    pub max_workspaces: Option<i32>,
    pub created_at: chrono::DateTime<crate::time::Timezone>,
}

impl TenantRecord {
    pub(crate) fn render(
        ctx: &loco_rs::app::AppContext,
        tenant: crate::models::tenant_tenants::Model,
    ) -> Self {
        Self {
            id: tenant.id,
            name: tenant.name,
            max_workspaces: tenant.max_workspaces,
            created_at: crate::time::render(ctx, tenant.created_at),
        }
    }
}
