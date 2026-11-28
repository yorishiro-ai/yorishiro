//! MCP tools for workspaces.

use crate::controllers::mcp::{render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use super::{YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::dtos::workspaces::CreateWorkspaceRequest;
use crate::models::{tenancy, workspace_workspaces};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceArgs {
    pub workspace_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateWorkspaceArgs {
    pub workspace_id: Uuid,
    pub name: Option<String>,
    #[serde(default)]
    pub replace_max_entities: bool,
    pub max_entities: Option<i32>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_workspaces)]
impl YorishiroMcpServer {
    #[tool(description = "List workspaces in the current tenant that the caller can read.")]
    pub(crate) async fn list_workspaces(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match workspace_workspaces::list_for_actor(&self.app_context().db, &auth).await {
            Ok(records) => ok_json(
                records
                    .into_iter()
                    .map(|record| workspace_workspaces::WorkspaceRecord {
                        created_at: crate::data::time::render(
                            self.app_context(),
                            &record.created_at,
                        )
                        .fixed_offset(),
                        ..record
                    })
                    .collect::<Vec<_>>(),
            ),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "Get workspace details, counts and effective entity cap (requires the workspace:GetWorkspace action)"
    )]
    pub(crate) async fn get_workspace(
        &self,
        Parameters(args): Parameters<WorkspaceArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        render(
            workspace_workspaces::detail_for_actor(self.app_context(), &auth, args.workspace_id)
                .await,
        )
    }

    #[tool(
        description = "Create a workspace in the current tenant (requires the workspace:CreateWorkspace action)."
    )]
    pub(crate) async fn create_workspace(
        &self,
        Parameters(args): Parameters<CreateWorkspaceRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match tenancy::create_workspace_in_context(
            self.app_context(),
            &auth,
            &args.name,
            args.max_entities,
        )
        .await
        {
            Ok(workspace) => render(workspace_workspaces::WorkspaceRecord::try_from(workspace)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "Update a workspace name (requires the workspace:UpdateWorkspace action); replacing or clearing its manual entity cap also requires permission to update the tenant; schema binding is immutable here"
    )]
    pub(crate) async fn update_workspace(
        &self,
        Parameters(args): Parameters<UpdateWorkspaceArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        render(
            workspace_workspaces::update_as(
                &self.app_context().db,
                &auth,
                args.workspace_id,
                crate::dtos::workspaces::UpdateWorkspaceRequest {
                    name: args.name,
                    replace_max_entities: args.replace_max_entities,
                    max_entities: args.max_entities,
                },
            )
            .await,
        )
    }

    #[tool(
        description = "Permanently delete a workspace and its data. The tenant's last workspace cannot be deleted."
    )]
    pub(crate) async fn delete_workspace(
        &self,
        Parameters(args): Parameters<WorkspaceArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match tenancy::delete_workspace_as(&self.app_context().db, &auth, args.workspace_id).await {
            Ok(()) => ok_json(serde_json::json!({"deleted":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
