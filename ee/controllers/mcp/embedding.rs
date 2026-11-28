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

#[tool_router(vis = "pub(crate)", router = tool_router_embedding)]
impl YorishiroMcpServer {
    #[tool(
        description = "Read the workspace's embedding provider settings without its secret. Returns not found when the workspace uses the deployment default."
    )]
    pub(crate) async fn get_workspace_embedding_provider(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        match workspace_embedding_keys::describe_as(&self.app_context().db, &actor).await {
            Ok(Some(record)) => ok_json(record),
            Ok(None) => Ok(err_to_tool_result(YorishiroError::not_found(
                "no embedding provider configured for this workspace",
            ))),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "Set the workspace's embedding provider after checking the endpoint and vector dimensions with sample text. Changes to document embeddings require a reindex at the deployment's configured time. Query instruction changes alone do not require a reindex. The response omits the secret."
    )]
    pub(crate) async fn update_workspace_embedding_provider(
        &self,
        Parameters(input): Parameters<SetEmbeddingKeyRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        match workspace_embedding_keys::configure(self.app_context(), &actor, input).await {
            Ok(()) => ok_json(serde_json::json!({"saved":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "Remove the workspace's embedding provider override. Automatic reindexing uses the deployment default provider and follows the deployment schedule."
    )]
    pub(crate) async fn delete_workspace_embedding_provider(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        match workspace_embedding_keys::remove(self.app_context(), &actor).await {
            Ok(()) => ok_json(serde_json::json!({"reset":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
