use crate::controllers::mcp::{render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use super::{VerifyOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::dtos::caller_identity::CallerIdentityResponse;

#[tool_router(vis = "pub(crate)", router = tool_router_caller_identity)]
impl YorishiroMcpServer {
    #[tool(
        description = "Discover workspaces the caller can view within the authenticated tenant. Requires workspace:DiscoverWorkspaces."
    )]
    pub async fn discover_workspaces(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        render(crate::models::tenancy::discover_workspaces(&self.app_context().db, &auth).await)
    }

    #[tool(
        description = "Inspect the authenticated identity, tenant, and selected workspace without exposing credentials."
    )]
    pub async fn get_caller_identity(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        match super::verify(self.app_context(), &parts).await? {
            VerifyOutcome::Verified(auth) => {
                match crate::models::iam::access::identity(&self.app_context().db, &auth).await {
                    Ok(identity) => ok_json(CallerIdentityResponse::from(identity)),
                    Err(error) => Ok(err_to_tool_result(error)),
                }
            }
            VerifyOutcome::Denied(denied) => Ok(denied),
        }
    }
}
