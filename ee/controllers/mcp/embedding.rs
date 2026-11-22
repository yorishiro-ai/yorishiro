use crate::controllers::mcp::{YorishiroMcpServer, err_to_tool_result, ok_json, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use crate::edition::ee::dtos::embedding::SetEmbeddingKeyRequest;
use crate::edition::ee::models::workspace_embedding_keys;
use crate::error::YorishiroError;
use crate::models::api_keys::ApiKeyScope;

#[tool_router(vis = "pub(crate)", router = tool_router_embedding)]
impl YorishiroMcpServer {
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Read the selected workspace's embedding provider metadata without its API key (requires read scope and embedding_provider_get IAM permission); returns not found when the deployment default is used")]
    pub(crate) async fn get_embedding_provider(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "get_embedding_provider");
        match workspace_embedding_keys::describe_as(&self.app_context().db, &actor).await {
            Ok(Some(record)) => ok_json(record),
            Ok(None) => Ok(err_to_tool_result(YorishiroError::not_found(
                "no embedding provider configured for this workspace",
            ))),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Configure the selected workspace's OpenAI-compatible embedding provider (requires schema scope and embedding_provider_update IAM permission); document changes queue a full reindex atomically, query-instruction-only changes do not; the supplied API key is stored and never returned")]
    pub(crate) async fn set_embedding_provider(
        &self,
        Parameters(input): Parameters<SetEmbeddingKeyRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "set_embedding_provider");
        match workspace_embedding_keys::configure(self.app_context(), &actor, input).await {
            Ok(()) => ok_json(serde_json::json!({"saved":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Remove the selected workspace's embedding provider override and queue a full reindex atomically using the deployment default (requires schema scope and embedding_provider_delete IAM permission)")]
    pub(crate) async fn reset_embedding_provider(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "reset_embedding_provider");
        match workspace_embedding_keys::remove(self.app_context(), &actor).await {
            Ok(()) => ok_json(serde_json::json!({"reset":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
