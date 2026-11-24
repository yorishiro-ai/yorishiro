//! MCP tools for schemas: listing, creating, archiving and deleting versions, reading the active one, and the built-in templates.

use crate::controllers::mcp::{render, try_tool, verified};
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

use super::{YorishiroMcpServer, ok_json};
use crate::error::YorishiroError;
use crate::models::api_keys::ApiKeyScope;
use crate::models::schema_schemas;
use crate::models::schema_schemas::CreateSchemaSource;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct GetActiveSchemaArgs {
    pub name: String,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct GetSchemaByIdArgs {
    pub schema_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct GetTemplateArgs {
    pub template_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct GetEntityTypeJsonSchemaArgs {
    /// Name of the active schema.
    pub schema_name: String,
    #[expect(clippy::doc_markdown, reason = "MCP exposes this description verbatim")]
    /// entity_type name within that schema.
    pub entity_type: String,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct ListSchemasArgs {
    /// Maximum number of results (defaults to 50 if omitted).
    pub limit: Option<i64>,
    /// Number of records to skip (defaults to 0 if omitted).
    pub offset: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct CreateSchemaArgs {
    #[expect(clippy::doc_markdown, reason = "MCP exposes this description verbatim")]
    /// JSON object conforming to `MetaSchemaDefinition` (name/description/entity_types/relation_types).
    /// If a schema with the same name already exists, whether the change is breaking or non-breaking is detected automatically and it is registered as a new version.
    /// Mutually exclusive with `template_id`; exactly one of the two must be set.
    pub definition: Option<Value>,
    /// ID of a template to use as the definition instead of supplying one inline.
    /// A UUID names one from the tenant's own library (see `list_template_library`); anything else names a built-in (see `list_templates`).
    /// Mutually exclusive with `definition`; exactly one of the two must be set.
    pub template_id: Option<String>,
}

impl TryFrom<CreateSchemaArgs> for CreateSchemaSource {
    type Error = YorishiroError;

    fn try_from(args: CreateSchemaArgs) -> Result<Self, Self::Error> {
        match (args.definition, args.template_id) {
            (Some(_), Some(_)) => Err(YorishiroError::ValidationFailed {
                message: "definition and template_id are mutually exclusive".into(),
                details: vec![],
                hint: "Set exactly one of `definition` or `template_id`".into(),
            }),
            (None, None) => Err(YorishiroError::ValidationFailed {
                message: "one of definition or template_id is required".into(),
                details: vec![],
                hint: "Set exactly one of `definition` or `template_id`".into(),
            }),
            (Some(definition), None) => serde_json::from_value(definition)
                .map(Self::Definition)
                .map_err(|err| YorishiroError::ValidationFailed {
                    message: format!("invalid schema definition: {err}"),
                    details: vec![],
                    hint: "Check the structure of MetaSchemaDefinition (name/description/entity_types/relation_types)".into(),
                }),
            (None, Some(template_id)) => Ok(Self::Template { template_id }),
        }
    }
}

#[tool_router(vis = "pub(crate)", router = tool_router_schemas)]
impl YorishiroMcpServer {
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema),
        description = "Archive a schema version in the selected workspace without deleting entities, version history or fork references; no older version is reactivated and future same-name creation advances the version (requires schema scope and schema_archive IAM permission)"
    )]
    pub(crate) async fn archive_schema(
        &self,
        Parameters(args): Parameters<GetSchemaByIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "archive_schema");
        render(schema_schemas::archive_for_actor(self.app_context(), actor, args.schema_id).await)
    }
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema),
        description = "Delete an archived schema version that nothing references (no entities, snapshots, inference proposals, workspaces or forks); an active version or any remaining reference is refused with the blockers named (requires schema scope and schema_delete IAM permission)"
    )]
    pub(crate) async fn delete_schema(
        &self,
        Parameters(args): Parameters<GetSchemaByIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "delete_schema");
        try_tool!(
            schema_schemas::delete_for_actor(self.app_context(), actor, args.schema_id).await
        );
        ok_json(serde_json::json!({ "deleted": true }))
    }
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "List schema summaries across permitted workspaces, or narrow to a selected workspace. Includes archived versions and each row's workspace ID (requires read scope and schema_list IAM permission)"
    )]
    pub(crate) async fn list_schemas(
        &self,
        Parameters(args): Parameters<ListSchemasArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "list_schemas");
        let page = crate::models::pagination::ListParams::from(&args);
        let summaries =
            try_tool!(schema_schemas::list_for_actor(self.app_context(), &actor, page).await);
        ok_json(summaries)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Get the currently active schema definition by name (requires read scope and schema_get IAM permission)"
    )]
    pub async fn get_active_schema(
        &self,
        Parameters(args): Parameters<GetActiveSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "get_active_schema");
        render(schema_schemas::get_active_for_actor(self.app_context(), actor, &args.name).await)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Get a specific version of a schema definition by ID (requires read scope and schema_get IAM permission)"
    )]
    pub(crate) async fn get_schema_by_id(
        &self,
        Parameters(args): Parameters<GetSchemaByIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "get_schema_by_id");
        render(schema_schemas::get_by_id_for_actor(self.app_context(), actor, args.schema_id).await)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema),
        description = "Register a new schema, or add a new version to an existing schema \
                           (requires schema scope and schema_create IAM permission)"
    )]
    pub async fn create_schema(
        &self,
        Parameters(args): Parameters<CreateSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "create_schema");
        let source = try_tool!(CreateSchemaSource::try_from(args));
        render(schema_schemas::create_for_actor(self.app_context(), actor, source).await)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "List built-in schema templates that can be used as a starting point for \
                           create_schema instead of writing a definition from scratch (requires \
                           read scope)"
    )]
    pub async fn list_templates(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let _authorized = verified!(self, parts, "list_templates");

        ok_json(crate::data::templates::list_templates())
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Get a built-in schema template definition by ID (requires read scope)"
    )]
    pub(crate) async fn get_template(
        &self,
        Parameters(args): Parameters<GetTemplateArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let _authorized = verified!(self, parts, "get_template");
        render(crate::data::templates::get_template(&args.template_id))
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Get a specific entity_type within the active schema as a JSON Schema \
                           (requires read scope and schema_get IAM permission). Use this to let an agent learn field types, \
                           required fields, enums, etc. ahead of time."
    )]
    pub(crate) async fn get_entity_type_json_schema(
        &self,
        Parameters(args): Parameters<GetEntityTypeJsonSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "get_entity_type_json_schema");
        render(
            schema_schemas::entity_type_json_schema_for_actor(
                self.app_context(),
                actor,
                &args.schema_name,
                &args.entity_type,
            )
            .await,
        )
    }
}

crate::dtos::common::list_params_from!(ListSchemasArgs);
