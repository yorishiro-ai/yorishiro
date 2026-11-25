use crate::controllers::mcp::{YorishiroMcpServer, render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

#[tool_router(vis = "pub(crate)", router = tool_router_dashboard)]
impl YorishiroMcpServer {
    #[tool(
        description = "Read current tenant billing plan metadata, effective workspace cap, usage and member summary (requires tenant_get and tenant_user_list IAM permissions with a tenant-scoped credential); this read never changes billing or plan policy"
    )]
    pub(crate) async fn get_tenant_overview(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        render(
            crate::edition::ee::models::tenant_tenants::overview_as(self.app_context(), &auth)
                .await,
        )
    }
}
