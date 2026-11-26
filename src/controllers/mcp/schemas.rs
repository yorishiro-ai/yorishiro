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
use crate::models::schema_schemas;
use crate::models::schema_schemas::CreateSchemaSource;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct GetActiveSchemaArgs {
    pub schema_name: String,
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
    /// A UUID names a template library item (see `list_template_library_items`).
    /// Other identifiers name built-in templates (see `list_builtin_templates`).
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
        description = "Archive a schema version without deleting entities, history or fork references. This does not reactivate an older version."
    )]
    pub(crate) async fn archive_schema(
        &self,
        Parameters(args): Parameters<GetSchemaByIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(schema_schemas::archive_for_actor(self.app_context(), actor, args.schema_id).await)
    }
    #[tool(
        description = "Delete an archived schema version that nothing references. The error names any remaining references."
    )]
    pub(crate) async fn delete_schema(
        &self,
        Parameters(args): Parameters<GetSchemaByIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        try_tool!(
            schema_schemas::delete_for_actor(self.app_context(), actor, args.schema_id).await
        );
        ok_json(serde_json::json!({ "deleted": true }))
    }
    #[tool(
        description = "List schema summaries, including archived versions and each result's workspace ID."
    )]
    pub(crate) async fn list_schemas(
        &self,
        Parameters(args): Parameters<ListSchemasArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let page = crate::models::pagination::ListParams::from(&args);
        let summaries =
            try_tool!(schema_schemas::list_for_actor(self.app_context(), &actor, page).await);
        ok_json(summaries)
    }

    #[tool(description = "Get the active schema definition by name.")]
    pub async fn get_active_schema(
        &self,
        Parameters(args): Parameters<GetActiveSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            schema_schemas::get_active_for_actor(self.app_context(), actor, &args.schema_name)
                .await,
        )
    }

    #[tool(description = "Get a schema version by ID.")]
    pub(crate) async fn get_schema(
        &self,
        Parameters(args): Parameters<GetSchemaByIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(schema_schemas::get_by_id_for_actor(self.app_context(), actor, args.schema_id).await)
    }

    #[tool(description = "Register a schema or add a version to an existing schema.")]
    pub async fn create_schema(
        &self,
        Parameters(args): Parameters<CreateSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let source = try_tool!(CreateSchemaSource::try_from(args));
        render(schema_schemas::create_for_actor(self.app_context(), actor, source).await)
    }

    #[tool(
        description = "List built-in schema templates that can be used as a starting point for \
                           create_schema instead of writing a definition from scratch"
    )]
    pub async fn list_builtin_templates(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        if let Err(error) = crate::models::template_templates::authorize_builtin(
            &self.app_context().db,
            &auth,
            crate::models::iam::policy::Action::ListBuiltinTemplates,
        )
        .await
        {
            return render(Err::<(), _>(error));
        }
        ok_json(crate::data::templates::list_templates())
    }

    #[tool(description = "Get a built-in schema template definition by ID")]
    pub(crate) async fn get_builtin_template(
        &self,
        Parameters(args): Parameters<GetTemplateArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        render(
            async {
                crate::models::template_templates::authorize_builtin(
                    &self.app_context().db,
                    &auth,
                    crate::models::iam::policy::Action::GetBuiltinTemplate,
                )
                .await?;
                crate::data::templates::get_template(&args.template_id)
            }
            .await,
        )
    }

    #[tool(
        description = "Get the JSON Schema for an entity type in the active schema, including field types, required fields and enums."
    )]
    pub(crate) async fn get_entity_type_json_schema(
        &self,
        Parameters(args): Parameters<GetEntityTypeJsonSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
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
