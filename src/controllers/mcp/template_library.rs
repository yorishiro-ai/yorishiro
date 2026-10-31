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
#[serde(deny_unknown_fields)]
pub(crate) struct CreateTemplateLibraryArgs {
    pub name: String,
    pub description: Option<String>,
    pub definition: serde_json::Value,
    #[serde(default)]
    pub tags: Vec<String>,
    pub locale: Option<String>,
    pub author: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateTemplateLibraryArgs {
    pub id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
    pub definition: Option<serde_json::Value>,
    pub tags: Option<Vec<String>>,
    pub locale: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForkTemplateLibraryArgs {
    pub id: Uuid,
    pub name: String,
}

#[tool_router(vis = "pub(crate)", router = tool_router_template_library)]
impl YorishiroMcpServer {
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Create a tenant-shared schema template in the library, not a workspace data schema (requires read scope and tenant Admin or Owner)"
    )]
    pub(crate) async fn create_template_library_item(
        &self,
        Parameters(args): Parameters<CreateTemplateLibraryArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("create_template_library_item")?,
        )
        .await?
        {
            super::VerifyOutcome::Verified(auth) => auth,
            super::VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        let definition = match serde_json::from_value(args.definition) {
            Ok(value) => value,
            Err(error) => return Err(ErrorData::invalid_params(error.to_string(), None)),
        };
        match template_templates::create_as(
            &self.app_context().db,
            &auth,
            template_templates::CreateTemplateInput {
                name: args.name,
                description: args.description,
                definition,
                tags: args.tags,
                locale: args.locale,
                author: args.author,
            },
        )
        .await
        {
            Ok(template) => ok_json(template),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Update an owned tenant template; existing workspace schema versions are unchanged (requires read scope and tenant Admin or Owner)"
    )]
    pub(crate) async fn update_template_library_item(
        &self,
        Parameters(args): Parameters<UpdateTemplateLibraryArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("update_template_library_item")?,
        )
        .await?
        {
            super::VerifyOutcome::Verified(auth) => auth,
            super::VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        let definition = match args.definition.map(serde_json::from_value).transpose() {
            Ok(value) => value,
            Err(error) => return Err(ErrorData::invalid_params(error.to_string(), None)),
        };
        match template_templates::update_as(
            &self.app_context().db,
            &auth,
            args.id,
            template_templates::UpdateTemplateInput {
                name: args.name,
                description: args.description,
                definition,
                tags: args.tags,
                locale: args.locale,
            },
        )
        .await
        {
            Ok(template) => ok_json(template),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Delete an owned tenant template while preserving existing workspace schemas (requires read scope and tenant Admin or Owner)"
    )]
    pub(crate) async fn delete_template_library_item(
        &self,
        Parameters(args): Parameters<GetTemplateLibraryItemArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("delete_template_library_item")?,
        )
        .await?
        {
            super::VerifyOutcome::Verified(auth) => auth,
            super::VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match template_templates::delete_as(&self.app_context().db, &auth, args.id).await {
            Ok(()) => ok_json(serde_json::json!({"deleted": true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Fork a visible schema template into the current tenant's library (requires read scope and tenant Admin or Owner)"
    )]
    pub(crate) async fn fork_template_library_item(
        &self,
        Parameters(args): Parameters<ForkTemplateLibraryArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("fork_template_library_item")?,
        )
        .await?
        {
            super::VerifyOutcome::Verified(auth) => auth,
            super::VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match template_templates::fork_as(&self.app_context().db, &auth, args.id, args.name).await {
            Ok(template) => ok_json(template),
            Err(error) => Ok(err_to_tool_result(error)),
        }
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
        let authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("list_template_library")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let tenant_id = authorized.auth_context().tenant_id;
        let page = crate::models::pagination::ListParams::new(args.limit, args.offset);
        let templates =
            match template_templates::list_templates(&self.app_context().db, tenant_id, page).await
            {
                Ok(value) => value,
                Err(err) => return Ok(err_to_tool_result(err)),
            };
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
        let authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("get_template_library_item")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let tenant_id = authorized.auth_context().tenant_id;
        let template = match template_templates::get_template(
            &self.app_context().db,
            tenant_id,
            args.id,
        )
        .await
        {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        ok_json(template)
    }
}
