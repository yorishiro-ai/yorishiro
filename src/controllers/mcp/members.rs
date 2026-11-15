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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransferOwnershipArgs {
    pub successor_user_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetMemberRoleArgs {
    pub user_id: Uuid,
    pub role: tenant_memberships::MembershipRole,
}

#[tool_router(vis = "pub(crate)", router = tool_router_members)]
impl YorishiroMcpServer {
    #[tool(
        meta = super::scope_requirement(ApiKeyScope::Read),
        description = "Create a tenant user with a tenant-local user name and password; the user starts with no IAM permissions until policies are attached (requires read scope and tenant_user_create IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn create_tenant_user(
        &self,
        Parameters(input): Parameters<crate::dtos::members::CreateTenantUserRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("create_tenant_user")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match tenant_memberships::create_tenant_user_as(&self.app_context().db, &auth, &input).await
        {
            Ok(member) => ok_json(member),
            Err(error) => Ok(err_to_tool_result(error)),
        }
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("reset_tenant_user_password")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        description = "Change an existing tenant member's legacy role, returning existing credential metadata; IAM policies determine permissions and tenant root identity cannot be changed (requires read scope and tenant_user_update IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn set_member_role(
        &self,
        Parameters(args): Parameters<SetMemberRoleArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("set_member_role")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match tenant_memberships::update_member_access(
            &self.app_context().db,
            &auth,
            args.user_id,
            args.role,
        )
        .await
        {
            Ok(keys) => ok_json(
                serde_json::json!({"user_id": args.user_id, "role": args.role, "keys": keys}),
            ),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Preview a tenant member role change and its key/workspace impact without changing data (requires read scope and tenant_user_update IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn preview_member_role_change(
        &self,
        Parameters(args): Parameters<SetMemberRoleArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("preview_member_role_change")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match tenant_memberships::preview_role_change(
            &self.app_context().db,
            &auth,
            args.user_id,
            args.role,
        )
        .await
        {
            Ok(preview) => ok_json(preview),
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("list_members")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        let db = &self.app_context().db;
        let page = crate::models::pagination::ListParams::new(args.limit, args.offset);
        match tenant_memberships::list_members_as(db, &auth, page).await {
            Ok(members) => ok_json(members),
            Err(error) => Ok(err_to_tool_result(error)),
        }
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("preview_member_removal")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match tenant_memberships::preview_removal(&self.app_context().db, &auth, args.user_id).await
        {
            Ok(preview) => ok_json(preview),
            Err(error) => Ok(err_to_tool_result(error)),
        }
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("remove_member")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match tenant_memberships::remove_member_as(&self.app_context().db, &auth, args.user_id)
            .await
        {
            Ok(()) => ok_json(serde_json::json!({"removed": true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Reject tenant ownership transfer because root identity is immutable (requires read scope and tenant_user_update IAM permission)"
    )]
    pub(crate) async fn transfer_ownership(
        &self,
        Parameters(args): Parameters<TransferOwnershipArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("transfer_ownership")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match tenant_memberships::transfer_ownership(
            &self.app_context().db,
            &auth,
            args.successor_user_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"transferred": true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
