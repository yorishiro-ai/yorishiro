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
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema),
        description = "Bulk-import schemas/entities/relations from a JSON Lines document in the \
                           export format (requires schema scope, since importing schemas is itself \
                           a schema-scope-only operation). Returns a job_id; poll import_job_status. \
                           Requires import_create IAM permission at enqueue and execution. The worker applies every record in one transaction or rolls everything back."
    )]
    pub async fn import_jsonl(
        &self,
        Parameters(args): Parameters<ImportJsonlArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let required = self.required_scope("import_jsonl")?;
        let actor = verified!(super::verify(self.app_context(), &parts, required).await?);
        let authorized = try_tool!(
            super::authorize::Authorized::for_target(
                self.app_context(),
                &actor,
                required,
                crate::models::iam::access::WorkspaceTarget::Permission(
                    crate::models::iam::policy::Action::ImportCreate,
                ),
            )
            .await
        );

        let job_id = try_tool!(
            crate::workers::import::enqueue(
                self.app_context(),
                authorized.txn(),
                authorized.auth_context(),
                args.jsonl,
            )
            .await
        );
        authorized.commit().await?;
        ok_json(QueuedJobResponse { job_id })
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema),description = "Poll a queued JSON Lines import by ID (requires schema scope and import_job_get IAM permission)")]
    pub(crate) async fn import_job_status(
        &self,
        Parameters(args): Parameters<ImportJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let required = self.required_scope("import_job_status")?;
        let actor = verified!(super::verify(self.app_context(), &parts, required).await?);
        let mut authorized = try_tool!(
            super::authorize::Authorized::for_target(
                self.app_context(),
                &actor,
                required,
                crate::models::iam::access::WorkspaceTarget::Job(args.job_id),
            )
            .await
        );
        try_tool!(
            authorized
                .require_action(crate::models::iam::policy::Action::ImportJobGet)
                .await
        );
        render(
            crate::models::queue_job_lifecycles::bulk_job_status(
                authorized.txn(),
                authorized.workspace_id(),
                args.job_id,
                &["import"],
            )
            .await,
        )
    }
}
