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
        description = "Read the selected workspace's inference LLM endpoint and model without its API key (requires llm_provider_get IAM permission); returns not found when unconfigured"
    )]
    pub(crate) async fn get_llm_provider(
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
        description = "Configure the selected workspace's inference LLM endpoint, model and API key (requires llm_provider_update IAM permission); stores the secret without returning it or invoking inference"
    )]
    pub(crate) async fn set_llm_provider(
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
        description = "Remove the selected workspace's inference LLM credentials (requires llm_provider_delete IAM permission); future inference requires configuration again, and already-started external calls are not cancelled"
    )]
    pub(crate) async fn reset_llm_provider(
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
