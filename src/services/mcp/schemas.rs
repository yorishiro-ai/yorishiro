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

use super::{YorishiroMcpServer, authorized, err_to_tool_result, mcp_try, ok_json};
use crate::error::YorishiroError;
use crate::metaschema::MetaSchemaDefinition;
use crate::models::content_schemas;
use crate::services::auth::ApiKeyScope;

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
    /// JSON object conforming to `MetaSchemaDefinition` (name/description/entity_types/relation_types).
    /// If a schema with the same name already exists, whether the change is breaking or
    /// non-breaking is detected automatically and it is registered as a new version.
    pub definition: Value,
}

#[tool_router(vis = "pub(crate)", router = tool_router_schemas)]
impl YorishiroMcpServer {
    #[tool(
        description = "Get the currently active schema definition by name (requires read scope)"
    )]
    pub async fn get_active_schema(
        &self,
        Parameters(args): Parameters<GetActiveSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = authorized!(&self.ctx, &parts, ApiKeyScope::Read);

        let workspace_id = authorized.ctx.workspace_id;
        let record = mcp_try!(
            content_schemas::get_active_schema(authorized.txn(), workspace_id, &args.name).await
        );
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
        let authorized = authorized!(&self.ctx, &parts, ApiKeyScope::Read);

        let workspace_id = authorized.ctx.workspace_id;
        let record = mcp_try!(
            content_schemas::get_by_id(authorized.txn(), workspace_id, args.schema_id).await
        );
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
        let authorized = authorized!(&self.ctx, &parts, ApiKeyScope::Schema);

        let definition: MetaSchemaDefinition = match serde_json::from_value(args.definition) {
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
        };

        let tenant_id = authorized.ctx.tenant_id;
        let workspace_id = authorized.ctx.workspace_id;
        let (record, diff) = mcp_try!(
            content_schemas::create_schema(authorized.txn(), tenant_id, workspace_id, definition)
                .await
        );
        authorized.commit().await?;
        ok_json(serde_json::json!({
            "schema": record,
            "diff": diff,
        }))
    }
}
