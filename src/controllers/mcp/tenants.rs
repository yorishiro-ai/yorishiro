//! MCP tools for the current tenant: reading and updating it, and inspecting or starting its retirement.

use crate::controllers::mcp::{render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use super::{YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::dtos::tenants::DisableTenantRequest;
use crate::models::tenant_tenants::retirement;

#[tool_router(vis = "pub(crate)", router = tool_router_tenants)]
impl YorishiroMcpServer {
    #[tool(
        description = "Read the current tenant's basic metadata and stored manual workspace cap (requires the tenant:GetTenant action)"
    )]
    pub(crate) async fn get_tenant(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match crate::models::tenant_tenants::detail_as(&self.app_context().db, &auth).await {
            Ok(tenant) => ok_json(crate::dtos::tenants::TenantRecord::render(
                self.app_context(),
                tenant,
            )),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "Update current tenant name or explicitly replace or clear its manual workspace cap (requires the tenant:UpdateTenant action)"
    )]
    pub(crate) async fn update_tenant(
        &self,
        Parameters(input): Parameters<crate::dtos::tenants::UpdateTenantRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match crate::models::tenant_tenants::update_as(&self.app_context().db, &auth, input).await {
            Ok(tenant) => ok_json(crate::dtos::tenants::TenantRecord::render(
                self.app_context(),
                tenant,
            )),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "Inspect the current tenant's retirement inventory and retention consequences without changing data"
    )]
    pub(crate) async fn get_tenant_retirement_inventory(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        render(retirement::inventory_as(&self.app_context().db, &auth).await)
    }

    #[tool(
        description = "Confirm the tenant UUID to disable it, revoke its credentials and cancel its work. Data, identities and quota usage remain."
    )]
    pub(crate) async fn disable_tenant(
        &self,
        Parameters(args): Parameters<DisableTenantRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match retirement::disable_as(&self.app_context().db, &auth, args.confirmed_tenant_id).await
        {
            Ok(()) => ok_json(serde_json::json!({"disabled": true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
