use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use crate::controllers::mcp::{
    VerifyOutcome, YorishiroMcpServer, err_to_tool_result, ok_json, verify,
};
use crate::edition::ee::dtos::inference::SetLlmKeyRequest;
use crate::edition::ee::models::workspace_llm_keys;
use crate::error::YorishiroError;
use crate::models::api_keys::ApiKeyScope;

#[tool_router(vis = "pub(crate)", router = tool_router_llm)]
impl YorishiroMcpServer {
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Read the selected workspace's inference LLM endpoint and model without its API key (requires read scope); returns not found when unconfigured")]
    pub(crate) async fn get_llm_provider(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match verify(
            self.app_context(),
            &parts,
            self.required_scope("get_llm_provider")?,
        )
        .await?
        {
            VerifyOutcome::Verified(actor) => actor,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_llm_keys::describe(&self.app_context().db, actor.workspace_id).await {
            Ok(Some(record)) => ok_json(record),
            Ok(None) => Ok(err_to_tool_result(YorishiroError::not_found(
                "no LLM credentials configured",
            ))),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Configure the selected workspace's inference LLM endpoint, model and API key (requires schema scope); stores the secret without returning it or invoking inference")]
    pub(crate) async fn set_llm_provider(
        &self,
        Parameters(input): Parameters<SetLlmKeyRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match verify(
            self.app_context(),
            &parts,
            self.required_scope("set_llm_provider")?,
        )
        .await?
        {
            VerifyOutcome::Verified(actor) => actor,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_llm_keys::set(
            &self.app_context().db,
            actor.workspace_id,
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

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Remove the selected workspace's inference LLM credentials (requires schema scope); future inference requires configuration again, and already-started external calls are not cancelled")]
    pub(crate) async fn reset_llm_provider(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match verify(
            self.app_context(),
            &parts,
            self.required_scope("reset_llm_provider")?,
        )
        .await?
        {
            VerifyOutcome::Verified(actor) => actor,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_llm_keys::clear(&self.app_context().db, actor.workspace_id).await {
            Ok(()) => ok_json(serde_json::json!({"reset":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
