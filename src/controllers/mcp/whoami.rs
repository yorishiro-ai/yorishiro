use crate::controllers::mcp::{render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use super::{VerifyOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::dtos::whoami::WhoAmIResponse;

#[tool_router(vis = "pub(crate)", router = tool_router_whoami)]
impl YorishiroMcpServer {
    #[tool(
        description = "Inspect the audit grant, tenant root identity and the visible tools; IAM is evaluated per operation and this snapshot does not authorize requests"
    )]
    pub async fn capabilities(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match crate::models::iam::access::capabilities(&self.app_context().db, &auth).await {
            Ok(capabilities) => {
                let tools = self.tool_capabilities(capabilities.audit)?;
                ok_json(serde_json::json!({
                    "audit": capabilities.audit,
                    "tenant_root": capabilities.tenant_root,
                    "tools": tools,
                    "constraints": {
                        "database_backend": capabilities.database_backend,
                        "row_level_security": capabilities.row_level_security,
                        "workspace_key_retargeting": capabilities.tenant_root,
                        "tenant_key_target_header": "X-Workspace-Id",
                        "workspace_selection_argument": "workspace_id",
                        "discovery_authorizes_requests": false,
                        "authorization_model": "tenant_iam",
                    },
                }))
            }
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "Discover workspace candidates within the current credential's tenant; workspace-bound credentials list only their bound workspace, and discovery does not grant access"
    )]
    pub async fn discover_workspaces(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        render(crate::models::tenancy::discover_workspaces(&self.app_context().db, &auth).await)
    }

    #[tool(
        description = "Inspect the current identity, tenant, selected workspace, effective scope and independent audit grant without exposing credentials"
    )]
    pub async fn whoami(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        match super::verify(self.app_context(), &parts).await? {
            VerifyOutcome::Verified(auth) => {
                match crate::models::iam::access::identity(&self.app_context().db, &auth).await {
                    Ok(identity) => ok_json(WhoAmIResponse::from(identity)),
                    Err(error) => Ok(err_to_tool_result(error)),
                }
            }
            VerifyOutcome::Denied(denied) => Ok(denied),
        }
    }
}
