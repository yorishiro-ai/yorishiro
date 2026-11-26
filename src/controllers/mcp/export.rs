use crate::controllers::mcp::{try_tool, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::model::CallToolResult;
use rmcp::model::ContentBlock;
use rmcp::{tool, tool_router};

use super::YorishiroMcpServer;
use crate::models::transfer::export;

#[tool_router(vis = "pub(crate)", router = tool_router_export)]
impl YorishiroMcpServer {
    #[tool(
        description = "Export schemas, entities, and relations as newline-delimited JSON for create_import_job. Limited to 8 MiB and 10,000 records."
    )]
    pub(crate) async fn export_workspace(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let lines = try_tool!(export::export_jsonl_for_actor(self.app_context(), actor).await);
        Ok(CallToolResult::success(vec![ContentBlock::text(lines)]))
    }
}
