use crate::controllers::mcp::{render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use crate::controllers::mcp::YorishiroMcpServer;
use crate::models::api_keys::ApiKeyScope;

#[tool_router(vis = "pub(crate)", router = tool_router_dashboard)]
impl YorishiroMcpServer {
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Read current tenant billing plan metadata, effective workspace cap, usage and member summary (requires read scope, tenant_get and tenant_user_list IAM permissions with a tenant-scoped credential); this read never changes billing or plan policy"
    )]
    pub(crate) async fn get_tenant_overview(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "get_tenant_overview");
        render(
            crate::edition::ee::models::tenant_tenants::overview_as(self.app_context(), &auth)
                .await,
        )
    }
}
