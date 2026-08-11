use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::metaschema::{self, MetaSchemaDefinition};
use yorishiro_core::repositories::schemas;
use yorishiro_core::services::auth::ApiKeyScope;
use yorishiro_core::templates;

use super::{YorishiroMcpServer, authorized, err_to_tool_result, mcp_try, ok_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetActiveSchemaArgs {
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSchemaByIdArgs {
    pub schema_id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateSchemaArgs {
    /// JSON object conforming to `MetaSchemaDefinition`
    /// (name/description/entity_types/relation_types). If a schema with the same
    /// name already exists, whether the change is breaking or non-breaking is
    /// detected automatically and it is registered as a new version. Mutually
    /// exclusive with `template_id`; exactly one of the two must be set.
    pub definition: Option<Value>,
    /// ID of a built-in template to use as the definition instead of supplying one
    /// inline. See `list_templates` for the available IDs. Mutually exclusive with
    /// `definition`; exactly one of the two must be set.
    pub template_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetEntityTypeJsonSchemaArgs {
    /// Name of the active schema.
    pub schema_name: String,
    /// entity_type name within that schema.
    pub entity_type: String,
}

#[tool_router(vis = "pub(crate)", router = tool_router_schemas)]
impl YorishiroMcpServer {
    #[tool(
        description = "List summaries of all schemas registered for the tenant (all versions, including archived). Use this to discover what schemas exist (requires read scope)"
    )]
    pub async fn list_schemas(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Read);

        let workspace_id = authorized.ctx.workspace_id;
        let summaries = mcp_try!(schemas::list(authorized.conn(), workspace_id).await);
        ok_json(summaries)
    }

    #[tool(
        description = "Get the currently active schema definition by name (requires read scope)"
    )]
    pub async fn get_active_schema(
        &self,
        Parameters(args): Parameters<GetActiveSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Read);

        let workspace_id = authorized.ctx.workspace_id;
        let record =
            mcp_try!(schemas::get_active_schema(authorized.conn(), workspace_id, &args.name).await);
        ok_json(record)
    }

    #[tool(
        description = "Get a specific version of a schema definition by ID (requires read scope)"
    )]
    pub async fn get_schema_by_id(
        &self,
        Parameters(args): Parameters<GetSchemaByIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Read);

        let workspace_id = authorized.ctx.workspace_id;
        let record =
            mcp_try!(schemas::get_by_id(authorized.conn(), workspace_id, args.schema_id).await);
        ok_json(record)
    }

    #[tool(
        description = "Register a new schema, or add a new version to an existing schema \
                           (requires schema scope)"
    )]
    pub async fn create_schema(
        &self,
        Parameters(args): Parameters<CreateSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Schema);

        let definition: MetaSchemaDefinition = match (args.definition, args.template_id) {
            (Some(_), Some(_)) => {
                return Ok(err_to_tool_result(YorishiroError::ValidationFailed {
                    message: "definition and template_id are mutually exclusive".into(),
                    details: vec![],
                    hint: "Set exactly one of `definition` or `template_id`".into(),
                }));
            }
            (None, None) => {
                return Ok(err_to_tool_result(YorishiroError::ValidationFailed {
                    message: "one of definition or template_id is required".into(),
                    details: vec![],
                    hint: "Set exactly one of `definition` or `template_id`".into(),
                }));
            }
            (Some(definition), None) => match serde_json::from_value(definition) {
                Ok(definition) => definition,
                Err(err) => {
                    return Ok(err_to_tool_result(YorishiroError::ValidationFailed {
                        message: format!("invalid schema definition: {err}"),
                        details: vec![],
                        hint: "Check the structure of MetaSchemaDefinition \
                               (name/description/entity_types/relation_types)"
                            .into(),
                    }));
                }
            },
            (None, Some(template_id)) => mcp_try!(templates::get_template(&template_id)),
        };

        let tenant_id = authorized.ctx.tenant_id;
        let workspace_id = authorized.ctx.workspace_id;
        let (record, diff) = mcp_try!(
            schemas::create_schema(authorized.conn(), tenant_id, workspace_id, definition).await
        );
        ok_json(serde_json::json!({
            "schema": record,
            "diff": diff,
        }))
    }

    #[tool(
        description = "List built-in schema templates that can be passed as `template_id` to \
                           `create_schema` instead of writing a definition from scratch \
                           (requires read scope)"
    )]
    pub async fn list_templates(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let _authorized = authorized!(&self.state, &parts, ApiKeyScope::Read);

        ok_json(templates::list_templates())
    }

    #[tool(
        description = "Get a specific entity_type within the active schema as a JSON Schema \
                           (requires read scope). Use this to let an agent learn field types, \
                           required fields, enums, etc. ahead of time."
    )]
    pub async fn get_entity_type_json_schema(
        &self,
        Parameters(args): Parameters<GetEntityTypeJsonSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Read);

        let workspace_id = authorized.ctx.workspace_id;
        let record = mcp_try!(
            schemas::get_active_schema(authorized.conn(), workspace_id, &args.schema_name).await
        );

        match record.definition.entity_types.get(&args.entity_type) {
            Some(entity_type_def) => {
                ok_json(metaschema::entity_type_to_json_schema(entity_type_def))
            }
            None => Ok(err_to_tool_result(YorishiroError::not_found(format!(
                "entity_type '{}' not found in schema '{}'",
                args.entity_type, args.schema_name
            )))),
        }
    }

    #[tool(
        description = "List schemas in this workspace whose origin template has been edited \
                           since the copy was taken (requires read scope). Reports only; the \
                           upstream edit is not applied, because doing so could invalidate \
                           entities already stored against the current definition."
    )]
    pub async fn list_upstream_changes(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = authorized!(&self.state, &parts, ApiKeyScope::Read);

        let workspace_id = authorized.ctx.workspace_id;
        let changes = mcp_try!(
            schemas::list_with_upstream_changes(&self.state.identity_pool, workspace_id).await
        );
        ok_json(changes)
    }

    #[tool(
        description = "Show what following the origin template would do to a schema \
                           (requires read scope). Compares the definition as copied, the \
                           template now, and this schema, and reports each differing field as \
                           auto_add, auto_update, keep_local or conflict. Nothing is written; \
                           a conflict is a question for a person."
    )]
    pub async fn merge_preview(
        &self,
        Parameters(args): Parameters<GetSchemaByIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Read);

        let tenant_id = authorized.ctx.tenant_id;
        let workspace_id = authorized.ctx.workspace_id;
        let plan = mcp_try!(
            schemas::merge_preview(
                authorized.conn(),
                &self.state.identity_pool,
                tenant_id,
                workspace_id,
                args.schema_id,
            )
            .await
        );
        ok_json(plan)
    }

    #[tool(
        description = "Follow the origin template: write the merged definition as the schema's \
                           next version (requires schema scope). Takes upstream's changes to \
                           fields this workspace has not touched, and keeps this workspace's \
                           own. Refuses if any field conflicts -- run merge_preview first and \
                           resolve those by editing the schema."
    )]
    pub async fn merge_apply(
        &self,
        Parameters(args): Parameters<GetSchemaByIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = authorized!(&self.state, &parts, ApiKeyScope::Schema);

        let tenant_id = authorized.ctx.tenant_id;
        let workspace_id = authorized.ctx.workspace_id;
        let (schema, diff) = mcp_try!(
            schemas::merge_apply(
                authorized.conn(),
                &self.state.identity_pool,
                tenant_id,
                workspace_id,
                args.schema_id,
            )
            .await
        );
        ok_json(serde_json::json!({ "schema": schema, "diff": diff }))
    }
}

#[cfg(test)]
#[path = "../../../tests/http/mcp/schemas.rs"]
mod tests;
