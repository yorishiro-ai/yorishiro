//! MCP tools for reading and changing which columns the Entities table shows per workspace and entity type.

use crate::controllers::mcp::{YorishiroMcpServer, ok_json, render, try_tool, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

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
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List the selected workspace's stored entity-table column preferences (requires read scope and entity_columns_get IAM permission); these display preferences are shared by workspace users")]
    pub(crate) async fn list_entity_columns(
        &self,
        Parameters(args): Parameters<ListColumnsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "list_entity_columns");
        render(
            entity_column_preferences::list_for_actor(
                self.app_context(),
                &actor,
                crate::models::pagination::ListParams::from(&args),
            )
            .await,
        )
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write), description = "Replace the selected workspace's visible columns for one entity type (requires write scope and entity_columns_update IAM permission); validates duplicate columns and the display limit without changing schema definitions")]
    pub(crate) async fn set_entity_columns(
        &self,
        Parameters(args): Parameters<SetColumnsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "set_entity_columns");
        render(
            entity_column_preferences::set_for_actor(
                self.app_context(),
                actor,
                &args.entity_type,
                &args.columns,
            )
            .await,
        )
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write), description = "Reset the selected workspace's entity-table columns for one entity type to schema-derived defaults (requires write scope and entity_columns_delete IAM permission); this removes only the display preference")]
    pub(crate) async fn reset_entity_columns(
        &self,
        Parameters(args): Parameters<ResetColumnsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "reset_entity_columns");
        try_tool!(
            entity_column_preferences::clear_for_actor(
                self.app_context(),
                actor,
                &args.entity_type
            )
            .await
        );
        ok_json(serde_json::json!({"reset": true}))
    }
}

crate::dtos::common::list_params_from!(ListColumnsArgs);
