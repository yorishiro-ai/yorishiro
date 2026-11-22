//! MCP tools for workspace schema forks: listing, creating, updating and deleting a fork of an upstream schema.

use crate::controllers::mcp::{render, try_tool, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use crate::controllers::mcp::{YorishiroMcpServer, ok_json};
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
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List schema forks across permitted workspaces, or narrow to a selected workspace (requires read scope and schema_fork_list IAM permission)")]
    pub(crate) async fn list_schema_forks(
        &self,
        Parameters(page): Parameters<crate::dtos::iam::ListIam>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(
            crate::controllers::mcp::verify(
                self.app_context(),
                &parts,
                self.required_scope("list_schema_forks")?,
            )
            .await?
        );
        render(
            model::list_for_actor(
                self.app_context(),
                &actor,
                crate::models::pagination::ListParams::new(page.limit, page.offset),
            )
            .await,
        )
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Read a schema fork owned by the selected workspace, including its immutable current head (requires read scope and schema_fork_get IAM permission)")]
    pub(crate) async fn get_schema_fork(
        &self,
        Parameters(args): Parameters<ForkIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let required = self.required_scope("get_schema_fork")?;
        let actor =
            verified!(crate::controllers::mcp::verify(self.app_context(), &parts, required).await?);
        let target = crate::models::iam::access::WorkspaceTarget::Workspace(try_tool!(
            model::workspace_for_actor(&self.app_context().db, &actor, args.fork_id).await
        ));
        let mut authorized = try_tool!(
            crate::controllers::mcp::authorize::Authorized::for_target(
                self.app_context(),
                &actor,
                required,
                target,
            )
            .await
        );
        try_tool!(
            authorized
                .require_action(crate::models::iam::policy::Action::SchemaForkGet)
                .await
        );

        let actor = authorized.auth_context();
        render(
            model::get(
                authorized.txn(),
                actor.tenant_id,
                authorized.workspace_id(),
                args.fork_id,
            )
            .await,
        )
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Create a workspace-owned fork of a same-tenant source schema (requires schema scope and schema_fork_create IAM permission); validates source identity and workspace cycles without modifying the source")]
    pub(crate) async fn create_schema_fork(
        &self,
        Parameters(input): Parameters<model::CreateInput>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let required = self.required_scope("create_schema_fork")?;
        let actor =
            verified!(crate::controllers::mcp::verify(self.app_context(), &parts, required).await?);
        let target = crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::SchemaForkCreate,
        );
        let mut authorized = try_tool!(
            crate::controllers::mcp::authorize::Authorized::for_target(
                self.app_context(),
                &actor,
                required,
                target,
            )
            .await
        );
        try_tool!(
            authorized
                .require_action(crate::models::iam::policy::Action::SchemaForkCreate)
                .await
        );

        let actor = authorized.auth_context();
        let record = try_tool!(
            model::create(
                authorized.txn(),
                actor.tenant_id,
                authorized.workspace_id(),
                input,
            )
            .await
        );
        authorized.commit().await?;
        ok_json(record)
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Update a selected-workspace schema fork with a definition or action=follow using the REST model's history and expected-head guards (requires schema scope and schema_fork_update IAM permission); force is required to discard local edits")]
    pub(crate) async fn update_schema_fork(
        &self,
        Parameters(args): Parameters<UpdateForkArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let required = self.required_scope("update_schema_fork")?;
        let actor =
            verified!(crate::controllers::mcp::verify(self.app_context(), &parts, required).await?);
        let target = crate::models::iam::access::WorkspaceTarget::Workspace(try_tool!(
            model::workspace_for_actor(&self.app_context().db, &actor, args.fork_id).await
        ));
        let mut authorized = try_tool!(
            crate::controllers::mcp::authorize::Authorized::for_target(
                self.app_context(),
                &actor,
                required,
                target,
            )
            .await
        );
        try_tool!(
            authorized
                .require_action(crate::models::iam::policy::Action::SchemaForkUpdate)
                .await
        );

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
        let record = try_tool!(
            model::update(
                authorized.txn(),
                actor.tenant_id,
                authorized.workspace_id(),
                args.fork_id,
                input,
            )
            .await
        );
        authorized.commit().await?;
        ok_json(record)
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Delete a selected-workspace fork association using the REST reference-preserving model (requires schema scope and schema_fork_delete IAM permission); does not delete the source schema")]
    pub(crate) async fn delete_schema_fork(
        &self,
        Parameters(args): Parameters<ForkIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let required = self.required_scope("delete_schema_fork")?;
        let actor =
            verified!(crate::controllers::mcp::verify(self.app_context(), &parts, required).await?);
        let target = crate::models::iam::access::WorkspaceTarget::Workspace(try_tool!(
            model::workspace_for_actor(&self.app_context().db, &actor, args.fork_id).await
        ));
        let mut authorized = try_tool!(
            crate::controllers::mcp::authorize::Authorized::for_target(
                self.app_context(),
                &actor,
                required,
                target,
            )
            .await
        );
        try_tool!(
            authorized
                .require_action(crate::models::iam::policy::Action::SchemaForkDelete)
                .await
        );

        let actor = authorized.auth_context();
        try_tool!(
            model::delete(
                authorized.txn(),
                actor.tenant_id,
                authorized.workspace_id(),
                args.fork_id,
            )
            .await
        );
        authorized.commit().await?;
        ok_json(serde_json::json!({"deleted":true}))
    }
}
