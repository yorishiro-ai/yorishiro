use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::controllers::mcp::{
    AuthzOutcome, YorishiroMcpServer, authorize, err_to_tool_result, ok_json,
};
use crate::edition::ee::models::entity_column_preferences;
use crate::models::api_keys::ApiKeyScope;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListColumnsArgs {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetColumnsArgs {
    pub entity_type: String,
    pub columns: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResetColumnsArgs {
    pub entity_type: String,
}

#[tool_router(vis = "pub(crate)", router = tool_router_entity_columns)]
impl YorishiroMcpServer {
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List the selected workspace's stored entity-table column preferences (requires read scope); these display preferences are shared by workspace users")]
    pub(crate) async fn list_entity_columns(
        &self,
        Parameters(args): Parameters<ListColumnsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match authorize(
            self.app_context(),
            &parts,
            self.required_scope("list_entity_columns")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match entity_column_preferences::list(
            authorized.txn(),
            authorized.auth_context().workspace_id,
            crate::models::pagination::ListParams::new(args.limit, args.offset),
        )
        .await
        {
            Ok(records) => ok_json(records),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write), description = "Replace the selected workspace's visible columns for one entity type (requires write scope); validates duplicate columns and the display limit without changing schema definitions")]
    pub(crate) async fn set_entity_columns(
        &self,
        Parameters(args): Parameters<SetColumnsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match authorize(
            self.app_context(),
            &parts,
            self.required_scope("set_entity_columns")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        let record = match entity_column_preferences::set(
            authorized.txn(),
            authorized.auth_context().workspace_id,
            &args.entity_type,
            &args.columns,
        )
        .await
        {
            Ok(record) => record,
            Err(error) => return Ok(err_to_tool_result(error)),
        };
        authorized.commit().await?;
        ok_json(record)
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write), description = "Reset the selected workspace's entity-table columns for one entity type to schema-derived defaults (requires write scope); this removes only the display preference")]
    pub(crate) async fn reset_entity_columns(
        &self,
        Parameters(args): Parameters<ResetColumnsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match authorize(
            self.app_context(),
            &parts,
            self.required_scope("reset_entity_columns")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        if let Err(error) = entity_column_preferences::clear(
            authorized.txn(),
            authorized.auth_context().workspace_id,
            &args.entity_type,
        )
        .await
        {
            return Ok(err_to_tool_result(error));
        }
        authorized.commit().await?;
        ok_json(serde_json::json!({"reset": true}))
    }
}
