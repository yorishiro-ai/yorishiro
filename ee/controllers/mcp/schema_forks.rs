use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use crate::controllers::mcp::{
    AuthzOutcome, YorishiroMcpServer, authorize, err_to_tool_result, ok_json,
};
use crate::edition::ee::models::workspace_schema_forks as model;
use crate::models::api_keys::ApiKeyScope;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForkIdArgs {
    pub fork_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateForkArgs {
    pub fork_id: Uuid,
    pub definition: Option<serde_json::Value>,
    pub action: Option<String>,
    #[serde(default)]
    pub force: bool,
    pub expected_fork_schema_id: Option<Uuid>,
    pub expected_source_schema_id: Option<Uuid>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_schema_forks)]
impl YorishiroMcpServer {
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List schema forks owned by the selected workspace (requires read scope); source visibility does not authorize changing the source workspace")]
    pub(crate) async fn list_schema_forks(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match authorize(
            self.app_context(),
            &parts,
            self.required_scope("list_schema_forks")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        let actor = authorized.auth_context();
        match model::list(authorized.txn(), actor.tenant_id, actor.workspace_id).await {
            Ok(records) => ok_json(records),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Read a schema fork owned by the selected workspace, including its immutable current head (requires read scope)")]
    pub(crate) async fn get_schema_fork(
        &self,
        Parameters(args): Parameters<ForkIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match authorize(
            self.app_context(),
            &parts,
            self.required_scope("get_schema_fork")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        let actor = authorized.auth_context();
        match model::get(
            authorized.txn(),
            actor.tenant_id,
            actor.workspace_id,
            args.fork_id,
        )
        .await
        {
            Ok(record) => ok_json(record),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Create a workspace-owned fork of a same-tenant source schema (requires schema scope); validates source identity and workspace cycles without modifying the source")]
    pub(crate) async fn create_schema_fork(
        &self,
        Parameters(input): Parameters<model::CreateInput>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match authorize(
            self.app_context(),
            &parts,
            self.required_scope("create_schema_fork")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        let actor = authorized.auth_context();
        let record =
            match model::create(authorized.txn(), actor.tenant_id, actor.workspace_id, input).await
            {
                Ok(record) => record,
                Err(error) => return Ok(err_to_tool_result(error)),
            };
        authorized.commit().await?;
        ok_json(record)
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Update a selected-workspace schema fork with a definition or action=follow using the REST model's history and expected-head guards (requires schema scope); force is required to discard local edits")]
    pub(crate) async fn update_schema_fork(
        &self,
        Parameters(args): Parameters<UpdateForkArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match authorize(
            self.app_context(),
            &parts,
            self.required_scope("update_schema_fork")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        let definition = args
            .definition
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?;
        let actor = authorized.auth_context();
        let input = model::UpdateInput {
            definition,
            action: args.action,
            force: args.force,
            expected_fork_schema_id: args.expected_fork_schema_id,
            expected_source_schema_id: args.expected_source_schema_id,
        };
        let record = match model::update(
            authorized.txn(),
            actor.tenant_id,
            actor.workspace_id,
            args.fork_id,
            input,
        )
        .await
        {
            Ok(record) => record,
            Err(error) => return Ok(err_to_tool_result(error)),
        };
        authorized.commit().await?;
        ok_json(record)
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Delete a selected-workspace fork association using the REST reference-preserving model (requires schema scope); does not delete the source schema")]
    pub(crate) async fn delete_schema_fork(
        &self,
        Parameters(args): Parameters<ForkIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match authorize(
            self.app_context(),
            &parts,
            self.required_scope("delete_schema_fork")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        let actor = authorized.auth_context();
        if let Err(error) = model::delete(
            authorized.txn(),
            actor.tenant_id,
            actor.workspace_id,
            args.fork_id,
        )
        .await
        {
            return Ok(err_to_tool_result(error));
        }
        authorized.commit().await?;
        ok_json(serde_json::json!({"deleted":true}))
    }
}
