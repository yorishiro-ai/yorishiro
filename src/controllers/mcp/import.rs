use crate::controllers::mcp::{render, try_tool, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{YorishiroMcpServer, ok_json};
use crate::dtos::jobs::QueuedJobResponse;

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
                           export format. Returns a job_id; poll import_job_status. \
                           Requires import_create IAM permission at enqueue and execution. The worker applies every record in one transaction or rolls everything back."
    )]
    pub async fn import_jsonl(
        &self,
        Parameters(args): Parameters<ImportJsonlArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let job_id = try_tool!(
            crate::workers::import::enqueue_for_actor(self.app_context(), actor, args.jsonl).await
        );
        ok_json(QueuedJobResponse { job_id })
    }

    #[tool(
        description = "Poll a queued JSON Lines import by ID (requires import_job_get IAM permission)"
    )]
    pub(crate) async fn import_job_status(
        &self,
        Parameters(args): Parameters<ImportJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            crate::models::queue_job_lifecycles::import_job_status_for_actor(
                self.app_context(),
                actor,
                args.job_id,
            )
            .await,
        )
    }
}
