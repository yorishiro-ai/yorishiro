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
use crate::models::{tenancy, workspace_workspaces};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceArgs {
    pub workspace_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateWorkspaceArgs {
    pub name: String,
    pub max_entities: Option<i32>,
    pub schema_id: Option<Uuid>,
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
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "List visible workspace records in the current tenant; discovery is not authority to retarget a bound credential (requires read scope)"
    )]
    pub(crate) async fn list_workspaces(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("list_workspaces")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_workspaces::list_for_actor(&self.app_context().db, &auth).await {
            Ok(records) => ok_json(
                records
                    .into_iter()
                    .map(|record| workspace_workspaces::WorkspaceRecord {
                        created_at: crate::time::in_timezone(
                            record.created_at,
                            crate::time::configured(self.app_context()),
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
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Get selected workspace details, counts and effective entity cap; cannot retarget a workspace-bound credential (requires read scope)"
    )]
    pub(crate) async fn get_workspace(
        &self,
        Parameters(args): Parameters<WorkspaceArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("get_workspace")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_workspaces::detail_for_actor(self.app_context(), &auth, args.workspace_id)
            .await
        {
            Ok(detail) => ok_json(detail),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Create a workspace in the current tenant without retargeting the current credential (requires read scope and tenant Admin or Owner)"
    )]
    pub(crate) async fn create_workspace(
        &self,
        Parameters(args): Parameters<CreateWorkspaceArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("create_workspace")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match tenancy::create_workspace_in_context(
            self.app_context(),
            &auth,
            &args.name,
            args.max_entities,
            args.schema_id,
        )
        .await
        {
            Ok(workspace) => match workspace_workspaces::WorkspaceRecord::try_from(workspace) {
                Ok(record) => ok_json(record),
                Err(error) => Ok(err_to_tool_result(error)),
            },
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Update selected workspace name (workspace Admin) or replace/clear manual entity cap (tenant Admin); schema binding is immutable here (requires read scope)"
    )]
    pub(crate) async fn update_workspace(
        &self,
        Parameters(args): Parameters<UpdateWorkspaceArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("update_workspace")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_workspaces::update_as(
            &self.app_context().db,
            &auth,
            args.workspace_id,
            crate::dtos::workspaces::UpdateWorkspaceRequest {
                name: args.name,
                replace_max_entities: args.replace_max_entities,
                max_entities: args.max_entities,
            },
        )
        .await
        {
            Ok(record) => ok_json(record),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Permanently delete a current-tenant workspace and its data; the last workspace is protected (requires read scope and tenant Admin or Owner)"
    )]
    pub(crate) async fn delete_workspace(
        &self,
        Parameters(args): Parameters<WorkspaceArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("delete_workspace")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match tenancy::delete_workspace_as(&self.app_context().db, &auth, args.workspace_id).await {
            Ok(()) => ok_json(serde_json::json!({"deleted":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
