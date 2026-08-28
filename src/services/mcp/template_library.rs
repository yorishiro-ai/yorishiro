use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use super::{AuthzOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::models::identity_templates;
use crate::services::auth::ApiKeyScope;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetTemplateLibraryItemArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTemplateLibraryArgs {
    /// Maximum number of results (defaults to 50 if omitted).
    pub limit: Option<i64>,
    /// Number of records to skip (defaults to 0 if omitted).
    pub offset: Option<i64>,
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
        Parameters(args): Parameters<ListTemplateLibraryArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(&self.ctx, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let tenant_id = authorized.ctx.tenant_id;
        let page = crate::models::pagination::ListParams::new(args.limit, args.offset);
        let templates =
            match identity_templates::list_templates(&self.ctx.db, tenant_id, page).await {
                Ok(value) => value,
                Err(err) => return Ok(err_to_tool_result(err)),
            };
        ok_json(templates)
    }

    #[tool(
        description = "Get a single template from the tenant's DB-backed template library by \
                           ID (requires read scope)"
    )]
    pub async fn get_template_library_item(
        &self,
        Parameters(args): Parameters<GetTemplateLibraryItemArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(&self.ctx, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let tenant_id = authorized.ctx.tenant_id;
        let template =
            match identity_templates::get_template(&self.ctx.db, tenant_id, args.id).await {
                Ok(value) => value,
                Err(err) => return Ok(err_to_tool_result(err)),
            };
        ok_json(template)
    }
}
