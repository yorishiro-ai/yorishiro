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
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Inspect current grants and visible tools' required scopes and scope satisfaction; this snapshot does not guarantee operation success or authorize later requests (requires read scope)"
    )]
    pub async fn capabilities(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("capabilities")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::access::capabilities(&self.app_context().db, &auth).await {
            Ok(capabilities) => {
                let tools = self.tool_capabilities(&capabilities.scopes)?;
                ok_json(serde_json::json!({
                    "scopes": capabilities.scopes,
                    "audit": capabilities.audit,
                    "tenant_admin": capabilities.tenant_admin,
                    "tools": tools,
                }))
            }
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Discover workspace candidates within the current credential's tenant; service credentials list only the selected workspace, and discovery does not grant access (requires read scope)"
    )]
    pub async fn discover_workspaces(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("discover_workspaces")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::tenancy::discover_workspaces(&self.app_context().db, &auth).await {
            Ok(workspaces) => ok_json(workspaces),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Inspect the current identity, tenant, selected workspace, effective scope and independent audit grant without exposing credentials (requires read scope)"
    )]
    pub async fn whoami(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        match super::verify(self.app_context(), &parts, self.required_scope("whoami")?).await? {
            VerifyOutcome::Verified(auth) => {
                match crate::models::access::identity(&self.app_context().db, &auth).await {
                    Ok(identity) => ok_json(WhoAmIResponse::from(identity)),
                    Err(error) => Ok(err_to_tool_result(error)),
                }
            }
            VerifyOutcome::ScopeDenied(denied) => Ok(denied),
        }
    }
}
