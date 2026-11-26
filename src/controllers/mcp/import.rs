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
use crate::dtos::jobs::{ImportJobAccepted, ImportJobStatus};

#[derive(Deserialize, JsonSchema)]
pub(crate) struct ImportJsonlArgs {
    /// One `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line, as returned by `export_workspace`.
    pub jsonl: String,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct ImportJobArgs {
    import_job_id: uuid::Uuid,
}

#[tool_router(vis = "pub(crate)", router = tool_router_import)]
impl YorishiroMcpServer {
    #[tool(
        description = "Bulk-import schemas/entities/relations from a JSON Lines document in the \
                           export format. Poll the returned job ID with get_import_job. \
                           The worker applies every record in one transaction or rolls everything back."
    )]
    pub async fn create_import_job(
        &self,
        Parameters(args): Parameters<ImportJsonlArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let import_job_id = try_tool!(
            crate::workers::import::enqueue_for_actor(self.app_context(), actor, args.jsonl).await
        );
        ok_json(ImportJobAccepted { import_job_id })
    }

    #[tool(description = "Poll a queued JSON Lines import by ID")]
    pub(crate) async fn get_import_job(
        &self,
        Parameters(args): Parameters<ImportJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            crate::models::queue_job_lifecycles::import_job_status_for_actor(
                self.app_context(),
                actor,
                args.import_job_id,
            )
            .await
            .map(|status| ImportJobStatus {
                import_job_id: args.import_job_id,
                status,
            }),
        )
    }
}
