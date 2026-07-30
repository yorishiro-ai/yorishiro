use http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use yorishiro_core::repositories::import;
use yorishiro_core::services::auth::ApiKeyScope;

use super::{YorishiroMcpServer, authorized, mcp_try, ok_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportJsonlArgs {
    /// JSON Lines document in the same format `export_jsonl`/`GET /api/export.jsonl`
    /// produces: one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per
    /// line, newline-separated.
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
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Schema);

        let workspace_id = authorized.ctx.workspace_id;
        let result = mcp_try!(
            import::import_jsonl(authorized.conn(), workspace_id, args.jsonl.as_bytes()).await
        );
        ok_json(result)
    }
}
