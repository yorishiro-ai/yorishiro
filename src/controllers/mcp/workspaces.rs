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
pub(crate) struct WorkspaceMemberArgs {
    pub workspace_id: Uuid,
    pub user_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GrantWorkspaceMemberArgs {
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub role: crate::models::workspace_memberships::WorkspaceRole,
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
        description = "List direct workspace grants; inherited tenant administrator access is not a direct grant (requires read scope and workspace_get IAM permission)"
    )]
    pub(crate) async fn list_workspace_members(
        &self,
        Parameters(args): Parameters<WorkspaceArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("list_workspace_members")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::workspace_memberships::Model::list_as(
            &self.app_context().db,
            &auth,
            args.workspace_id,
        )
        .await
        {
            Ok(records) => ok_json(records),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Grant or change a direct workspace role for an existing tenant member; grants no tenant administration or key issuance authority (requires read scope and workspace_update IAM permission)"
    )]
    pub(crate) async fn grant_workspace_member(
        &self,
        Parameters(args): Parameters<GrantWorkspaceMemberArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("grant_workspace_member")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::workspace_memberships::Model::grant_as(
            &self.app_context().db,
            &auth,
            args.workspace_id,
            args.user_id,
            args.role,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"granted":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Revoke a direct workspace grant and affected workspace-bound credentials; inherited tenant authority remains (requires read scope and workspace_update IAM permission)"
    )]
    pub(crate) async fn revoke_workspace_member(
        &self,
        Parameters(args): Parameters<WorkspaceMemberArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("revoke_workspace_member")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::workspace_memberships::Model::revoke_as(
            &self.app_context().db,
            &auth,
            args.workspace_id,
            args.user_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"revoked":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "List current-tenant workspaces allowed by workspace_get IAM permission (requires read scope, workspace_list IAM permission and a tenant-scoped credential)"
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
                        created_at: crate::time::render(self.app_context(), record.created_at)
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
        description = "Get workspace details, counts and effective entity cap with workspace_get IAM permission; cannot retarget a selected or credential-bound workspace (requires read scope)"
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
        description = "Create a workspace in the current tenant without retargeting the current credential (requires read scope, a tenant-scoped credential and workspace_create IAM permission)"
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
        description = "Update a workspace name with workspace_update IAM permission; replacing or clearing its manual entity cap also requires tenant_update and a tenant-scoped credential; schema binding is immutable here (requires read scope)"
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
        description = "Permanently delete a current-tenant workspace and its data; the last workspace is protected (requires read scope and workspace_delete IAM permission within the credential workspace binding)"
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
