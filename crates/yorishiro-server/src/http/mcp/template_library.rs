use http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;
use yorishiro_core::repositories::tenancy;
use yorishiro_core::services::auth::ApiKeyScope;

use super::{YorishiroMcpServer, mcp_try, ok_json, verified};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetTemplateLibraryItemArgs {
    pub id: Uuid,
}

#[tool_router(vis = "pub(crate)", router = tool_router_template_library)]
impl YorishiroMcpServer {
    #[tool(
        description = "List the tenant's DB-backed schema template library (own templates plus \
                           any community-visible ones). Distinct from `list_templates`, which \
                           lists the built-in templates shipped with the server (requires read \
                           scope)"
    )]
    pub async fn list_template_library(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let ctx = verified!(&self.state, &parts, ApiKeyScope::Read);

        let templates =
            mcp_try!(tenancy::list_templates(&self.state.identity_pool, ctx.tenant_id).await);
        ok_json(templates)
    }

    #[tool(
        description = "Get a single template from the tenant's DB-backed template library by ID \
                           (requires read scope)"
    )]
    pub async fn get_template_library_item(
        &self,
        Parameters(args): Parameters<GetTemplateLibraryItemArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let ctx = verified!(&self.state, &parts, ApiKeyScope::Read);

        let template = mcp_try!(
            tenancy::get_template(&self.state.identity_pool, ctx.tenant_id, args.id).await
        );
        ok_json(template)
    }
}

#[cfg(test)]
#[path = "../../../tests/http/mcp/template_library.rs"]
mod tests;
