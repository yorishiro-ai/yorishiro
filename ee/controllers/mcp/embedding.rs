use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use crate::controllers::mcp::{
    VerifyOutcome, YorishiroMcpServer, err_to_tool_result, ok_json, verify,
};
use crate::edition::ee::dtos::embedding::SetEmbeddingKeyRequest;
use crate::edition::ee::models::workspace_embedding_keys;
use crate::error::YorishiroError;
use crate::models::api_keys::ApiKeyScope;

#[tool_router(vis = "pub(crate)", router = tool_router_embedding)]
impl YorishiroMcpServer {
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Read the selected workspace's embedding provider metadata without its API key (requires read scope); returns not found when the deployment default is used")]
    pub(crate) async fn get_embedding_provider(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match verify(
            self.app_context(),
            &parts,
            self.required_scope("get_embedding_provider")?,
        )
        .await?
        {
            VerifyOutcome::Verified(actor) => actor,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_embedding_keys::describe(&self.app_context().db, actor.workspace_id).await {
            Ok(Some(record)) => ok_json(record),
            Ok(None) => Ok(err_to_tool_result(YorishiroError::not_found(
                "no embedding provider configured for this workspace",
            ))),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Configure the selected workspace's OpenAI-compatible embedding provider and queue a full reindex atomically (requires schema scope); the supplied API key is stored and never returned")]
    pub(crate) async fn set_embedding_provider(
        &self,
        Parameters(input): Parameters<SetEmbeddingKeyRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match verify(
            self.app_context(),
            &parts,
            self.required_scope("set_embedding_provider")?,
        )
        .await?
        {
            VerifyOutcome::Verified(actor) => actor,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_embedding_keys::configure(self.app_context(), actor.workspace_id, input)
            .await
        {
            Ok(()) => ok_json(serde_json::json!({"saved":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Remove the selected workspace's embedding provider override and queue a full reindex atomically using the deployment default (requires schema scope)")]
    pub(crate) async fn reset_embedding_provider(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match verify(
            self.app_context(),
            &parts,
            self.required_scope("reset_embedding_provider")?,
        )
        .await?
        {
            VerifyOutcome::Verified(actor) => actor,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match workspace_embedding_keys::remove(self.app_context(), actor.workspace_id).await {
            Ok(()) => ok_json(serde_json::json!({"reset":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
