use crate::controllers::mcp::{YorishiroMcpServer, err_to_tool_result, ok_json, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use crate::edition::ee::dtos::worker_class::SetWorkerClassRequest;
use crate::edition::ee::models::workspace_worker_classes;
use crate::error::YorishiroError;

#[tool_router(vis = "pub(crate)", router = tool_router_worker_class)]
impl YorishiroMcpServer {
    #[tool(
        description = "Read the selected workspace's explicit worker-class assignment (requires worker_class_get IAM permission); missing assignments use shared compute and return not found here"
    )]
    pub(crate) async fn get_workspace_worker_class(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        match workspace_worker_classes::describe_as(&self.app_context().db, &actor).await {
            Ok(Some(record)) => ok_json(record),
            Ok(None) => Ok(err_to_tool_result(YorishiroError::not_found(
                "no worker class assignment configured for this workspace",
            ))),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "Assign the selected workspace's worker class (requires worker_class_update IAM permission); the deployment must declare the destination worker target, which does not guarantee a worker is online"
    )]
    pub(crate) async fn update_workspace_worker_class(
        &self,
        Parameters(input): Parameters<SetWorkerClassRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        match workspace_worker_classes::set_as(self.app_context(), &actor, input.worker_class).await
        {
            Ok(()) => ok_json(serde_json::json!({"saved":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "Remove the selected workspace's worker-class assignment and fall back to shared compute (requires worker_class_delete IAM permission); queued jobs retain their existing routing"
    )]
    pub(crate) async fn delete_workspace_worker_class(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        match workspace_worker_classes::clear_as(&self.app_context().db, &actor).await {
            Ok(()) => ok_json(serde_json::json!({"reset":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
