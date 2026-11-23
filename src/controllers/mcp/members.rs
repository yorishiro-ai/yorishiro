//! MCP tools for tenant membership: creating tenant users, resetting their passwords and previewing or removing members.

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
use crate::models::api_keys::ApiKeyScope;
use crate::models::tenant_memberships;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListMembersArgs {
    pub(crate) limit: Option<i64>,
    pub(crate) offset: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MemberArgs {
    pub user_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResetTenantUserPasswordArgs {
    pub user_id: Uuid,
    /// At least 12 characters. Revokes every credential issued to the user.
    pub password: String,
}

#[tool_router(vis = "pub(crate)", router = tool_router_members)]
impl YorishiroMcpServer {
    #[tool(
        meta = super::scope_requirement(ApiKeyScope::Read),
        description = "Create a tenant user with a tenant-local user name and password; the user starts with no IAM permissions until policies or groups are attached (requires read scope and tenant_user_create IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn create_tenant_user(
        &self,
        Parameters(input): Parameters<crate::dtos::members::CreateTenantUserRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "create_tenant_user");
        render(
            tenant_memberships::create_tenant_user_as(&self.app_context().db, &auth, &input).await,
        )
    }

    #[tool(
        meta = super::scope_requirement(ApiKeyScope::Read),
        description = "Reset a tenant user's password and revoke all their credentials; tenant root recovery is CLI-only (requires read scope and tenant_user_password_reset IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn reset_tenant_user_password(
        &self,
        Parameters(input): Parameters<ResetTenantUserPasswordArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "reset_tenant_user_password");
        match tenant_memberships::reset_tenant_user_password(
            &self.app_context().db,
            &auth,
            input.user_id,
            &input.password,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"reset": true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "List members of the current tenant (requires read scope and tenant_user_list IAM permission with a tenant-scoped credential; service credentials are rejected)"
    )]
    pub(crate) async fn list_members(
        &self,
        Parameters(args): Parameters<ListMembersArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "list_members");
        let db = &self.app_context().db;
        let page = crate::models::pagination::ListParams::new(args.limit, args.offset);
        render(tenant_memberships::list_members_as(db, &auth, page).await)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Preview tenant membership removal and credential revocation without changing data (requires read scope and tenant_user_delete IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn preview_member_removal(
        &self,
        Parameters(args): Parameters<MemberArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "preview_member_removal");
        render(
            tenant_memberships::preview_removal(&self.app_context().db, &auth, args.user_id).await,
        )
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Remove a current-tenant membership and revoke attributed tenant credentials; a tenant user's account is deleted with its membership, and tenant root removal is rejected (requires read scope and tenant_user_delete IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn remove_member(
        &self,
        Parameters(args): Parameters<MemberArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "remove_member");
        match tenant_memberships::remove_member_as(&self.app_context().db, &auth, args.user_id)
            .await
        {
            Ok(()) => ok_json(serde_json::json!({"removed": true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
