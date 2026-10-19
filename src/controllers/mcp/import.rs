use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{AuthzOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::dtos::jobs::QueuedJobResponse;
use crate::models::api_keys::ApiKeyScope;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct ImportJsonlArgs {
    /// JSON Lines document in the same format `export_jsonl`/`GET /api/export.jsonl` produces: one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line, newline-separated.
    pub jsonl: String,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct ImportJobArgs {
    job_id: uuid::Uuid,
}

#[tool_router(vis = "pub(crate)", router = tool_router_import)]
impl YorishiroMcpServer {
    #[tool(
        description = "Bulk-import schemas/entities/relations from a JSON Lines document in the \
                           export format (requires schema scope, since importing schemas is itself \
                           a schema-scope-only operation). Returns a job_id; poll import_job_status. \
                           The worker applies every record in one transaction or rolls everything back."
    )]
    pub async fn import_jsonl(
        &self,
        Parameters(args): Parameters<ImportJsonlArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized =
            match super::authorize(self.app_context(), &parts, ApiKeyScope::Schema).await? {
                AuthzOutcome::Authorized(authorized) => authorized,
                AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
            };

        let job_id = match crate::workers::import::enqueue(
            self.app_context(),
            authorized.txn(),
            authorized.auth_context(),
            args.jsonl,
        )
        .await
        {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        authorized.commit().await?;
        ok_json(QueuedJobResponse { job_id })
    }

    #[tool(description = "Poll a queued JSON Lines import (requires schema scope)")]
    pub(crate) async fn import_job_status(
        &self,
        Parameters(args): Parameters<ImportJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized =
            match super::authorize(self.app_context(), &parts, ApiKeyScope::Schema).await? {
                AuthzOutcome::Authorized(value) => value,
                AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
            };
        match crate::models::queue_job_lifecycles::bulk_job_status(
            authorized.txn(),
            authorized.auth_context().workspace_id,
            args.job_id,
            &["import"],
        )
        .await
        {
            Ok(status) => ok_json(status),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
