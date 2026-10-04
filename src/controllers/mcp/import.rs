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
use crate::models::api_keys::ApiKeyScope;
use crate::models::import;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct ImportJsonlArgs {
    /// JSON Lines document in the same format `export_jsonl`/`GET /api/export.jsonl` produces: one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line, newline-separated.
    pub jsonl: String,
}

#[tool_router(vis = "pub(crate)", router = tool_router_import)]
impl YorishiroMcpServer {
    #[tool(
        description = "Bulk-import schemas/entities/relations from a JSON Lines document in the \
                           export format (requires schema scope, since importing schemas is itself \
                           a schema-scope-only operation). Runs as a single transaction: either \
                           every record in `jsonl` is applied, or the first error rolls back \
                           everything imported so far."
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

        let tenant_id = authorized.auth_context().tenant_id;
        let workspace_id = authorized.auth_context().workspace_id;
        let imported_by = authorized.auth_context().user_id;
        let result = match import::import_jsonl(
            authorized.txn(),
            tenant_id,
            workspace_id,
            imported_by,
            args.jsonl.as_bytes(),
        )
        .await
        {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        authorized.commit().await?;
        ok_json(result)
    }
}
