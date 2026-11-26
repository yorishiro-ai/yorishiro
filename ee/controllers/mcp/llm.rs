use crate::controllers::mcp::{YorishiroMcpServer, err_to_tool_result, ok_json, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use crate::edition::ee::dtos::inference::SetLlmKeyRequest;
use crate::edition::ee::models::workspace_llm_keys;
use crate::error::YorishiroError;

#[tool_router(vis = "pub(crate)", router = tool_router_llm)]
impl YorishiroMcpServer {
    #[tool(
        description = "Read the workspace's inference endpoint and model without its secret. Returns not found when no provider is configured."
    )]
    pub(crate) async fn get_workspace_llm_provider(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        match workspace_llm_keys::describe_as(&self.app_context().db, &actor).await {
            Ok(Some(record)) => ok_json(record),
            Ok(None) => Ok(err_to_tool_result(YorishiroError::not_found(
                "no LLM credentials configured",
            ))),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "Set the workspace's inference endpoint, model and secret. The response omits the secret."
    )]
    pub(crate) async fn update_workspace_llm_provider(
        &self,
        Parameters(input): Parameters<SetLlmKeyRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        match workspace_llm_keys::set_as(
            &self.app_context().db,
            &actor,
            &input.base_url,
            &input.model,
            &input.api_key,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"saved":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "Remove the workspace's inference provider settings. Calls already in progress continue."
    )]
    pub(crate) async fn delete_workspace_llm_provider(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        match workspace_llm_keys::clear_as(&self.app_context().db, &actor).await {
            Ok(()) => ok_json(serde_json::json!({"reset":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
