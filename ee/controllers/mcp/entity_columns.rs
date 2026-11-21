//! MCP tools for reading and changing which columns the Entities table shows per workspace and entity type.

use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::controllers::mcp::{YorishiroMcpServer, err_to_tool_result, ok_json};
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
        let required = self.required_scope("list_entity_columns")?;
        let actor =
            match crate::controllers::mcp::verify(self.app_context(), &parts, required).await? {
                crate::controllers::mcp::VerifyOutcome::Verified(actor) => actor,
                crate::controllers::mcp::VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
            };
        match entity_column_preferences::list_for_actor(
            self.app_context(),
            &actor,
            crate::models::pagination::ListParams::new(args.limit, args.offset),
        )
        .await
        {
            Ok(records) => ok_json(records),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write), description = "Replace the selected workspace's visible columns for one entity type (requires write scope and entity_columns_update IAM permission); validates duplicate columns and the display limit without changing schema definitions")]
    pub(crate) async fn set_entity_columns(
        &self,
        Parameters(args): Parameters<SetColumnsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let required = self.required_scope("set_entity_columns")?;
        let actor =
            match crate::controllers::mcp::verify(self.app_context(), &parts, required).await? {
                crate::controllers::mcp::VerifyOutcome::Verified(actor) => actor,
                crate::controllers::mcp::VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
            };
        let target = crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::EntityColumnsUpdate,
        );
        let mut authorized = match crate::controllers::mcp::authorize::Authorized::for_target(
            self.app_context(),
            &actor,
            required,
            target,
        )
        .await
        {
            Ok(authorized) => authorized,
            Err(error) => return Ok(err_to_tool_result(error)),
        };
        if let Err(error) = authorized
            .require_action(crate::models::iam::policy::Action::EntityColumnsUpdate)
            .await
        {
            return Ok(err_to_tool_result(error));
        }

        let record = match entity_column_preferences::set(
            authorized.txn(),
            authorized.workspace_id(),
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

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write), description = "Reset the selected workspace's entity-table columns for one entity type to schema-derived defaults (requires write scope and entity_columns_delete IAM permission); this removes only the display preference")]
    pub(crate) async fn reset_entity_columns(
        &self,
        Parameters(args): Parameters<ResetColumnsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let required = self.required_scope("reset_entity_columns")?;
        let actor =
            match crate::controllers::mcp::verify(self.app_context(), &parts, required).await? {
                crate::controllers::mcp::VerifyOutcome::Verified(actor) => actor,
                crate::controllers::mcp::VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
            };
        let target = crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::EntityColumnsDelete,
        );
        let mut authorized = match crate::controllers::mcp::authorize::Authorized::for_target(
            self.app_context(),
            &actor,
            required,
            target,
        )
        .await
        {
            Ok(authorized) => authorized,
            Err(error) => return Ok(err_to_tool_result(error)),
        };
        if let Err(error) = authorized
            .require_action(crate::models::iam::policy::Action::EntityColumnsDelete)
            .await
        {
            return Ok(err_to_tool_result(error));
        }

        if let Err(error) = entity_column_preferences::clear(
            authorized.txn(),
            authorized.workspace_id(),
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
