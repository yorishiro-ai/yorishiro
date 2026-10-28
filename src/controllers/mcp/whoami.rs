use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use super::{VerifyOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::dtos::whoami::WhoAmIResponse;
use crate::models::api_keys::ApiKeyScope;

#[tool_router(vis = "pub(crate)", router = tool_router_whoami)]
impl YorishiroMcpServer {
    #[tool(
        description = "Discover workspace candidates within the current credential's tenant; service credentials list only the selected workspace, and discovery does not grant access (requires read scope)"
    )]
    pub async fn discover_workspaces(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(self.app_context(), &parts, ApiKeyScope::Read).await? {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::tenancy::discover_workspaces(&self.app_context().db, &auth).await {
            Ok(workspaces) => ok_json(workspaces),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

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
