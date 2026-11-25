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
        description = "Export schemas, entities, and relations in the selected workspace as newline-delimited JSON (requires export IAM permission); one {\"kind\":\"schema\"|\"entity\"|\"relation\",\"record\":{...}} object per line, the same format import_jsonl reads back. Limited to 8 MiB and 10,000 records; larger workspaces must use paginated list APIs."
    )]
    pub(crate) async fn export_jsonl(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let lines = try_tool!(export::export_jsonl_for_actor(self.app_context(), actor).await);
        Ok(CallToolResult::success(vec![ContentBlock::text(lines)]))
    }
}
