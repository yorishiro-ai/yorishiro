//! MCP tools for the workspace template library: create, update, delete, list, get and fork.

use crate::controllers::mcp::{render, try_tool, verified};
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

use super::{YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::dtos::template_library::{
    CreateTemplateRequest, ForkTemplateRequest, UpdateTemplateRequest,
};
use crate::models::api_keys::ApiKeyScope;
use crate::models::template_templates;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct GetTemplateLibraryItemArgs {
    pub id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct ListTemplateLibraryArgs {
    /// Maximum number of results (defaults to 50 if omitted).
    pub limit: Option<i64>,
    /// Number of records to skip (defaults to 0 if omitted).
    pub offset: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct UpdateTemplateLibraryArgs {
    pub id: Uuid,
    #[serde(flatten)]
    pub body: UpdateTemplateRequest,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct ForkTemplateLibraryArgs {
    pub id: Uuid,
    #[serde(flatten)]
    pub body: ForkTemplateRequest,
}

#[tool_router(vis = "pub(crate)", router = tool_router_template_library)]
impl YorishiroMcpServer {
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Create a tenant-shared schema template in the library, not a workspace data schema (requires read scope and template_library_create IAM permission)"
    )]
    pub(crate) async fn create_template_library_item(
        &self,
        Parameters(args): Parameters<CreateTemplateRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "create_template_library_item");
        render(template_templates::create_as(&self.app_context().db, &auth, args.into()).await)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Update an owned tenant template; existing workspace schema versions are unchanged (requires read scope and template_library_update IAM permission)"
    )]
    pub(crate) async fn update_template_library_item(
        &self,
        Parameters(args): Parameters<UpdateTemplateLibraryArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "update_template_library_item");
        render(
            template_templates::update_as(&self.app_context().db, &auth, args.id, args.body.into())
                .await,
        )
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Delete an owned tenant template while preserving existing workspace schemas (requires read scope and template_library_delete IAM permission)"
    )]
    pub(crate) async fn delete_template_library_item(
        &self,
        Parameters(args): Parameters<GetTemplateLibraryItemArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "delete_template_library_item");
        match template_templates::delete_as(&self.app_context().db, &auth, args.id).await {
            Ok(()) => ok_json(serde_json::json!({"deleted": true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Fork a visible schema template into the current tenant's library (requires read scope and template_library_fork IAM permission)"
    )]
    pub(crate) async fn fork_template_library_item(
        &self,
        Parameters(args): Parameters<ForkTemplateLibraryArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "fork_template_library_item");
        render(
            template_templates::fork_as(&self.app_context().db, &auth, args.id, args.body.name)
                .await,
        )
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "List the tenant's DB-backed schema template library (own templates plus \
                           any community-visible ones). Distinct from `list_templates`, which \
                           lists the built-in templates shipped with the server (requires read \
                           scope)"
    )]
    pub(crate) async fn list_template_library(
        &self,
        Parameters(args): Parameters<ListTemplateLibraryArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "list_template_library");

        let page = crate::models::pagination::ListParams::from(&args);
        let templates =
            try_tool!(template_templates::list_as(&self.app_context().db, &auth, page).await);
        ok_json(templates)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Get a single template from the tenant's DB-backed template library by \
                           ID (requires read scope)"
    )]
    pub(crate) async fn get_template_library_item(
        &self,
        Parameters(args): Parameters<GetTemplateLibraryItemArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "get_template_library_item");

        let template =
            try_tool!(template_templates::get_as(&self.app_context().db, &auth, args.id).await);
        ok_json(template)
    }
}

crate::dtos::common::list_params_from!(ListTemplateLibraryArgs);
