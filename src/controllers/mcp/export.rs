use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::model::CallToolResult;
use rmcp::model::ContentBlock;
use rmcp::{tool, tool_router};

use super::{AuthzOutcome, YorishiroMcpServer, err_to_tool_result};
use crate::models::api_keys::ApiKeyScope;
use crate::models::export;

#[tool_router(vis = "pub(crate)", router = tool_router_export)]
impl YorishiroMcpServer {
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Export every schema, entity, and relation in the selected workspace as newline-delimited JSON (requires read scope); one {\"kind\":\"schema\"|\"entity\"|\"relation\",\"record\":{...}} object per line, the same format import_jsonl reads back. The result can be large for a busy workspace."
    )]
    pub(crate) async fn export_jsonl(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("export_jsonl")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let records = match export::export_all(
            authorized.txn(),
            authorized.auth_context().workspace_id,
        )
        .await
        {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        let mut lines = String::new();
        for record in &records {
            lines.push_str(&serde_json::to_string(record).map_err(crate::error::mcp_internal)?);
            lines.push('\n');
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(lines)]))
    }
}
