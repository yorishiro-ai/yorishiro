use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use super::{VerifyOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::models::api_keys::ApiKeyScope;
use crate::models::tenant_tenants::retirement;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DisableTenantArgs {
    /// Must equal the current tenant UUID, not its display name.
    pub confirmed_tenant_id: Uuid,
}

#[tool_router(vis = "pub(crate)", router = tool_router_tenants)]
impl YorishiroMcpServer {
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Inspect current-tenant retirement inventory and retention consequences without changing data (requires read scope and tenant Owner)"
    )]
    pub(crate) async fn tenant_retirement_inventory(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("tenant_retirement_inventory")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match retirement::inventory_as(&self.app_context().db, &auth).await {
            Ok(inventory) => ok_json(inventory),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Disable the current tenant after confirming its UUID, revoking all its credentials and cancelling work; data and identities remain and quota stays occupied (requires read scope and tenant Owner; physical deletion is operator-only)"
    )]
    pub(crate) async fn disable_tenant(
        &self,
        Parameters(args): Parameters<DisableTenantArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("disable_tenant")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match retirement::disable_as(&self.app_context().db, &auth, args.confirmed_tenant_id).await
        {
            Ok(()) => ok_json(serde_json::json!({"disabled": true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
