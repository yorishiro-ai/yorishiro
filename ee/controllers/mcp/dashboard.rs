use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use crate::controllers::mcp::{VerifyOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::models::api_keys::ApiKeyScope;

#[tool_router(vis = "pub(crate)", router = tool_router_dashboard)]
impl YorishiroMcpServer {
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Read current tenant billing plan metadata, effective workspace cap, usage and member summary (requires read scope and tenant Admin or Owner); this read never changes billing or plan policy"
    )]
    pub(crate) async fn get_tenant_overview(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match crate::controllers::mcp::verify(
            self.app_context(),
            &parts,
            self.required_scope("get_tenant_overview")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::edition::ee::models::tenant_tenants::overview_as(self.app_context(), &auth)
            .await
        {
            Ok(overview) => ok_json(overview),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
