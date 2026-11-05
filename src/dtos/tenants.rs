use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct DisableTenantRequest {
    /// Explicitly confirms the authenticated owner's tenant UUID.
    pub(crate) confirmed_tenant_id: Uuid,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct UpdateTenantRequest {
    pub(crate) name: Option<String>,
    /// Explicitly replace the manual workspace cap, including clearing it with null.
    #[serde(default)]
    pub(crate) replace_max_workspaces: bool,
    pub(crate) max_workspaces: Option<i32>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct TenantRecord {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) max_workspaces: Option<i32>,
    pub(crate) created_at: chrono::DateTime<crate::time::Timezone>,
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
