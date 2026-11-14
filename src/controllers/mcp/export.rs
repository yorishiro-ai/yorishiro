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
        description = "Export schemas, entities, and relations in the selected workspace as newline-delimited JSON (requires read scope and export IAM permission); one {\"kind\":\"schema\"|\"entity\"|\"relation\",\"record\":{...}} object per line, the same format import_jsonl reads back. Limited to 8 MiB and 10,000 records; larger workspaces must use paginated list APIs."
    )]
    pub(crate) async fn export_jsonl(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("export_jsonl")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        if let Err(error) = authorized
            .require_action(crate::models::iam_policy::Action::Export)
            .await
        {
            return Ok(err_to_tool_result(error));
        }

        let lines = match export::export_jsonl(authorized.txn(), authorized.workspace_id()).await {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(lines)]))
    }
}
