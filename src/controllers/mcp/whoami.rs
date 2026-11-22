use crate::controllers::mcp::{render, verified};
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
        description = "Inspect credential scope, independent audit grant, tenant root identity and visible tools' scope requirements; IAM is evaluated per operation and this snapshot does not authorize requests (requires read scope)"
    )]
    pub async fn capabilities(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "capabilities");
        match crate::models::iam::access::capabilities(&self.app_context().db, &auth).await {
            Ok(capabilities) => {
                let tools = self.tool_capabilities(&capabilities.scopes, capabilities.audit)?;
                ok_json(serde_json::json!({
                    "scopes": capabilities.scopes,
                    "audit": capabilities.audit,
                    "tenant_root": capabilities.tenant_root,
                    "credential_scope": capabilities.credential_scope,
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
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Discover workspace candidates within the current credential's tenant; workspace-bound credentials list only their bound workspace, and discovery does not grant access (requires read scope)"
    )]
    pub async fn discover_workspaces(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "discover_workspaces");
        render(crate::models::tenancy::discover_workspaces(&self.app_context().db, &auth).await)
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
                match crate::models::iam::access::identity(&self.app_context().db, &auth).await {
                    Ok(identity) => ok_json(WhoAmIResponse::from(identity)),
                    Err(error) => Ok(err_to_tool_result(error)),
                }
            }
            VerifyOutcome::ScopeDenied(denied) => Ok(denied),
        }
    }
}
