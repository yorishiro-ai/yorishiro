use axum::http::request::Parts;
use chrono::{DateTime, Utc};
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use super::{VerifyOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::dtos::members::{CreateInviteResponse, InviteRecord};
use crate::models::api_keys::ApiKeyScope;
use crate::models::{tenant_memberships::MembershipRole, workspace_invites};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateInviteArgs {
    pub email: String,
    pub role: MembershipRole,
    pub expires_at: DateTime<Utc>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InviteArgs {
    pub invite_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AcceptInviteArgs {
    pub invite_token: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReissueInviteArgs {
    pub invite_id: Uuid,
    pub expires_at: DateTime<Utc>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_invites)]
impl YorishiroMcpServer {
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Accept a tenant invitation as the authenticated human account (requires read scope and matching account email); service credentials cannot accept, no login credential is issued and the selected workspace does not change")]
    pub(crate) async fn accept_invite(
        &self,
        Parameters(args): Parameters<AcceptInviteArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("accept_invite")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_invites::accept_authenticated_user(
            &self.app_context().db,
            &auth,
            &args.invite_token,
        )
        .await
        {
            Ok((user, invite)) => ok_json(crate::dtos::auth::AcceptInviteResponse {
                user_id: user.id,
                tenant_id: invite.tenant_id,
                role: invite.role,
            }),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Create an expiring tenant invitation and return its token for caller delivery, without sending email (requires read scope and tenant Admin or Owner; inviting an Owner requires Owner)"
    )]
    pub(crate) async fn create_invite(
        &self,
        Parameters(args): Parameters<CreateInviteArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("create_invite")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_invites::create_invite_as(
            &self.app_context().db,
            &auth,
            &args.email,
            args.role,
            args.expires_at,
        )
        .await
        {
            Ok((invite, token)) => ok_json(CreateInviteResponse {
                invite: InviteRecord::render(self.app_context(), invite),
                token,
            }),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "List current-tenant invitations without exposing their tokens (requires read scope and tenant Admin or Owner)"
    )]
    pub(crate) async fn list_invites(
        &self,
        Parameters(args): Parameters<super::members::ListMembersArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("list_invites")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        let page = crate::models::pagination::ListParams::new(args.limit, args.offset);
        match workspace_invites::list_invites(&self.app_context().db, &auth, page).await {
            Ok(invites) => ok_json(
                invites
                    .into_iter()
                    .map(|invite| InviteRecord::render(self.app_context(), invite))
                    .collect::<Vec<_>>(),
            ),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Cancel an unused current-tenant invitation; repeated cancellation is safe (requires read scope and tenant Admin or Owner)"
    )]
    pub(crate) async fn revoke_invite(
        &self,
        Parameters(args): Parameters<InviteArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("revoke_invite")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_invites::revoke_invite_as(&self.app_context().db, &auth, args.invite_id)
            .await
        {
            Ok(()) => ok_json(serde_json::json!({"revoked": true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Replace an unused current-tenant invitation token and expiry, invalidating the old token, for caller delivery (requires read scope and tenant Admin or Owner)"
    )]
    pub(crate) async fn reissue_invite(
        &self,
        Parameters(args): Parameters<ReissueInviteArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("reissue_invite")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_invites::reissue_invite_as(
            &self.app_context().db,
            &auth,
            args.invite_id,
            args.expires_at,
        )
        .await
        {
            Ok((invite, token)) => ok_json(CreateInviteResponse {
                invite: InviteRecord::render(self.app_context(), invite),
                token,
            }),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
