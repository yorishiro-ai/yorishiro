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
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Add an existing user to the current tenant or change their tenant role, returning existing key grant metadata (requires read scope and tenant Admin or Owner; Owner changes require Owner)"
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
        description = "Preview a tenant member role change and its key/workspace impact without changing data (requires read scope and tenant Admin or Owner)"
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
        description = "List members of the current tenant (requires read scope and tenant Admin or Owner; service credentials are rejected)"
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
        if let Err(error) =
            crate::models::access::require_tenant_admin(db, auth.tenant_id, auth.user_id).await
        {
            return Ok(err_to_tool_result(error));
        }
        let page = crate::models::pagination::ListParams::new(args.limit, args.offset);
        match tenant_memberships::list_members(db, auth.tenant_id, page).await {
            Ok(members) => ok_json(members),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Preview tenant membership removal and credential revocation without changing data (requires read scope and tenant Admin or Owner)"
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
        description = "Remove a current-tenant membership and revoke attributed tenant credentials, retaining the global user account (requires read scope and tenant Admin or Owner; last Owner is protected)"
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
        description = "Atomically transfer tenant ownership to an existing member and demote the caller to Admin (requires read scope and tenant Owner)"
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
