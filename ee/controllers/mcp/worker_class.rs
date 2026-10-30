use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use crate::controllers::mcp::{
    VerifyOutcome, YorishiroMcpServer, err_to_tool_result, ok_json, verify,
};
use crate::edition::ee::dtos::worker_class::SetWorkerClassRequest;
use crate::edition::ee::models::workspace_worker_classes;
use crate::error::YorishiroError;
use crate::models::api_keys::ApiKeyScope;

#[tool_router(vis = "pub(crate)", router = tool_router_worker_class)]
impl YorishiroMcpServer {
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Read the selected workspace's explicit worker-class assignment (requires read scope); missing assignments use shared compute and return not found here")]
    pub(crate) async fn get_worker_class(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match verify(
            self.app_context(),
            &parts,
            self.required_scope("get_worker_class")?,
        )
        .await?
        {
            VerifyOutcome::Verified(actor) => actor,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_worker_classes::describe(&self.app_context().db, actor.workspace_id).await {
            Ok(Some(record)) => ok_json(record),
            Ok(None) => Ok(err_to_tool_result(YorishiroError::not_found(
                "no worker class assignment configured for this workspace",
            ))),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Assign the selected workspace's worker class (requires schema scope); the deployment must declare the destination worker target, which does not guarantee a worker is online")]
    pub(crate) async fn set_worker_class(
        &self,
        Parameters(input): Parameters<SetWorkerClassRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match verify(
            self.app_context(),
            &parts,
            self.required_scope("set_worker_class")?,
        )
        .await?
        {
            VerifyOutcome::Verified(actor) => actor,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_worker_classes::set(
            self.app_context(),
            actor.workspace_id,
            input.worker_class,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"saved":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Remove the selected workspace's worker-class assignment and fall back to shared compute (requires schema scope); queued jobs retain their existing routing")]
    pub(crate) async fn reset_worker_class(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match verify(
            self.app_context(),
            &parts,
            self.required_scope("reset_worker_class")?,
        )
        .await?
        {
            VerifyOutcome::Verified(actor) => actor,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_worker_classes::clear(&self.app_context().db, actor.workspace_id).await {
            Ok(()) => ok_json(serde_json::json!({"reset":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
