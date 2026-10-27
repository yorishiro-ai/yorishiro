use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use super::{VerifyOutcome, YorishiroMcpServer, ok_json};
use crate::dtos::whoami::WhoAmIResponse;
use crate::models::api_keys::ApiKeyScope;

#[tool_router(vis = "pub(crate)", router = tool_router_whoami)]
impl YorishiroMcpServer {
    #[tool(
        description = "Inspect the current identity, tenant, selected workspace, effective scope and independent audit grant without exposing credentials (requires read scope)"
    )]
    pub async fn whoami(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        match super::verify(self.app_context(), &parts, ApiKeyScope::Read).await? {
            VerifyOutcome::Verified(auth) => ok_json(WhoAmIResponse::from(auth)),
            VerifyOutcome::ScopeDenied(denied) => Ok(denied),
        }
    }
}
