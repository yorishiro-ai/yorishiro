//! MCP tools for workspace schema forks: listing, creating, updating and deleting a fork of an upstream schema.

use crate::controllers::mcp::{YorishiroMcpServer, ok_json, render, try_tool, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use crate::edition::ee::models::workspace_schema_forks as model;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForkIdArgs {
    pub fork_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateForkArgs {
    pub fork_id: Uuid,
    pub definition: Option<serde_json::Value>,
    pub action: Option<String>,
    #[serde(default)]
    pub force: bool,
    pub expected_fork_schema_id: Option<Uuid>,
    pub expected_source_schema_id: Option<Uuid>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_schema_forks)]
impl YorishiroMcpServer {
    #[tool(
        description = "List schema forks across permitted workspaces, or narrow to a selected workspace (requires the schema:ListSchemaForks action)"
    )]
    pub(crate) async fn list_schema_forks(
        &self,
        Parameters(page): Parameters<crate::dtos::iam::ListIam>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            model::list_for_actor(
                self.app_context(),
                &actor,
                crate::models::pagination::ListParams::from(&page),
            )
            .await,
        )
    }

    #[tool(
        description = "Read a schema fork owned by the selected workspace, including its immutable current head (requires the schema:GetSchemaFork action)"
    )]
    pub(crate) async fn get_schema_fork(
        &self,
        Parameters(args): Parameters<ForkIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(model::get_for_actor(self.app_context(), actor, args.fork_id).await)
    }

    #[tool(
        description = "Create a workspace-owned fork of a same-tenant source schema (requires the schema:CreateSchemaFork action); validates source identity and workspace cycles without modifying the source"
    )]
    pub(crate) async fn create_schema_fork(
        &self,
        Parameters(input): Parameters<model::CreateInput>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(model::create_for_actor(self.app_context(), actor, input).await)
    }

    #[tool(
        description = "Update a selected-workspace schema fork with a definition or action=follow using the REST model's history and expected-head guards (requires the schema:UpdateSchemaFork action); force is required to discard local edits"
    )]
    pub(crate) async fn update_schema_fork(
        &self,
        Parameters(args): Parameters<UpdateForkArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let definition = args
            .definition
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?;
        let input = model::UpdateInput {
            definition,
            action: args.action,
            force: args.force,
            expected_fork_schema_id: args.expected_fork_schema_id,
            expected_source_schema_id: args.expected_source_schema_id,
        };
        render(model::update_for_actor(self.app_context(), actor, args.fork_id, input).await)
    }

    #[tool(
        description = "Delete a selected-workspace fork association using the REST reference-preserving model (requires the schema:DeleteSchemaFork action); does not delete the source schema"
    )]
    pub(crate) async fn delete_schema_fork(
        &self,
        Parameters(args): Parameters<ForkIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        try_tool!(model::delete_for_actor(self.app_context(), actor, args.fork_id).await);
        ok_json(serde_json::json!({"deleted":true}))
    }
}
