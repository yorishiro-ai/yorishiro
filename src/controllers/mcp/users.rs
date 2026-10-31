use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use super::{VerifyOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::dtos::users::{UpdateUserRequest, UserRecord};
use crate::models::api_keys::ApiKeyScope;

#[tool_router(vis = "pub(crate)", router = tool_router_users)]
impl YorishiroMcpServer {
    #[tool(meta = super::scope_requirement(ApiKeyScope::Read), description = "Read your own global user profile without exposing password hashes or OAuth subjects; service credentials are denied (requires read scope)")]
    pub(crate) async fn get_current_user(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("get_current_user")?,
        )
        .await?
        {
            VerifyOutcome::Verified(actor) => actor,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::user_users::current_profile(&self.app_context().db, &actor).await {
            Ok(user) => ok_json(UserRecord::render(self.app_context(), user)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = super::scope_requirement(ApiKeyScope::Read), description = "Update or clear your own global display name across all tenants, not another user's identity; email, password and OAuth subjects cannot be changed here (requires read scope)")]
    pub(crate) async fn update_current_user(
        &self,
        Parameters(input): Parameters<UpdateUserRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("update_current_user")?,
        )
        .await?
        {
            VerifyOutcome::Verified(actor) => actor,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::user_users::update_profile(
            &self.app_context().db,
            &actor,
            input.display_name,
        )
        .await
        {
            Ok(user) => ok_json(UserRecord::render(self.app_context(), user)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
