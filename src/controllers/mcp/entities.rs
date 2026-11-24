//! MCP tools for entities, plus drift reports and the migration tools (dry run, fill defaults, status, reindex, undo).

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
use crate::dtos::entities::{CreateEntityRequest, FillDefaultsRequest, UpdateEntityRequest};
use crate::models::api_keys::ApiKeyScope;
use crate::models::entity_entities;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct GetEntityArgs {
    pub id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct UpdateEntityArgs {
    pub id: Uuid,
    #[serde(flatten)]
    pub body: UpdateEntityRequest,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct DeleteEntityArgs {
    pub id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct MigrationDryRunArgs {
    pub name: String,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct MigrationJobArgs {
    pub job_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListEntitiesArgs {
    pub entity_type: Option<String>,
    /// JSONB containment filter matched against entity data, e.g. `{"status": "active"}`.
    pub filter: Option<Value>,
    /// Restricts results to entities created against this schema version.
    pub schema_version: Option<i32>,
    /// Maximum number of results (defaults to 50 if omitted).
    pub limit: Option<i64>,
    /// Number of records to skip (defaults to 0 if omitted).
    pub offset: Option<i64>,
}

impl TryFrom<ListEntitiesArgs> for entity_entities::ListEntitiesQuery {
    type Error = crate::YorishiroError;

    fn try_from(args: ListEntitiesArgs) -> Result<Self, Self::Error> {
        let page = crate::models::pagination::ListParams::from(&args);
        Ok(Self {
            entity_type: args.entity_type,
            filter: args.filter,
            schema_version: args.schema_version,
            page,
        })
    }
}

#[tool_router(vis = "pub(crate)", router = tool_router_entities)]
impl YorishiroMcpServer {
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write),description = "Create a new entity (requires write scope and entity_create IAM permission)")]
    pub async fn create_entity(
        &self,
        Parameters(args): Parameters<CreateEntityRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "create_entity");
        let record = try_tool!(
            entity_entities::create_for_actor(self.app_context(), &actor, args.into(),).await
        );
        ok_json(record)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),description = "Get a single entity by ID (requires read scope and entity_get IAM permission)")]
    pub async fn get_entity(
        &self,
        Parameters(args): Parameters<GetEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "get_entity");
        let record =
            try_tool!(entity_entities::get_for_actor(self.app_context(), &actor, args.id).await);
        ok_json(record)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write),description = "Replace the data of an existing entity (requires write scope and entity_update IAM permission); pass expected_updated_at from a prior read to refuse the write with a conflict when the entity changed in between")]
    pub(crate) async fn update_entity(
        &self,
        Parameters(args): Parameters<UpdateEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "update_entity");
        let updated_by = actor.user_id;
        let record = try_tool!(
            entity_entities::update_for_actor(
                self.app_context(),
                &actor,
                entity_entities::UpdateEntityInput {
                    id: args.id,
                    data: args.body.data,
                    updated_by,
                    expected_updated_at: args.body.expected_updated_at,
                },
            )
            .await
        );
        ok_json(record)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write),description = "Delete an entity (requires write scope and entity_delete IAM permission)")]
    pub(crate) async fn delete_entity(
        &self,
        Parameters(args): Parameters<DeleteEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "delete_entity");
        try_tool!(entity_entities::delete_for_actor(self.app_context(), &actor, args.id).await);
        ok_json(serde_json::json!({ "deleted": true }))
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),description = "List entities (requires read scope and entity_list IAM permission)")]
    pub async fn list_entities(
        &self,
        Parameters(args): Parameters<ListEntitiesArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "list_entities");

        let query = try_tool!(args.try_into());

        let records =
            try_tool!(entity_entities::list_for_actor(self.app_context(), &actor, query).await);
        ok_json(records)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Report how an entity stands against the active version of its schema \
                           (requires read scope and entity_drift_get IAM permission). Entities are migrated lazily, so one written \
                           against an older version can still contain newer fields. Reports active \
                           fields absent from stored data, including nested fields in existing \
                           objects. Present nulls and type errors are not missing-value diagnostics."
    )]
    pub(crate) async fn get_entity_drift(
        &self,
        Parameters(args): Parameters<GetEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "get_entity_drift");
        render(entity_entities::drift_for_actor(self.app_context(), actor, args.id).await)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Count what migrating a schema's entities to its active version would \
                           face, without doing it (requires read scope and migration_dry_run IAM permission). Reports how many are \
                           current, how many are behind without missing required fields, and how many lack a \
                           field the active version requires: the last being the work a \
                           migration would have to fill in. Inspects stored data, not only schema \
                           definitions; this is not full active-schema validation."
    )]
    pub async fn migration_dry_run(
        &self,
        Parameters(args): Parameters<MigrationDryRunArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "migration_dry_run");
        render(
            entity_entities::migration_dry_run_for_actor(self.app_context(), actor, &args.name)
                .await,
        )
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Migration),
        description = "Enqueue a fill-defaults job (requires migration scope and migration_fill_defaults IAM permission, rechecked at execution). Returns a job ID; the worker fills absent required fields and takes undo snapshots. Completed counts appear in the audit log."
    )]
    pub async fn fill_defaults(
        &self,
        Parameters(args): Parameters<FillDefaultsRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "fill_defaults");
        let job_id = try_tool!(
            crate::workers::fill_defaults::enqueue_for_actor(
                self.app_context(),
                actor,
                args.schema_name.clone(),
            )
            .await
        );
        ok_json(crate::dtos::entities::FillDefaultsResponse {
            schema_name: args.schema_name,
            job_id,
        })
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Migration),
        description = "Get a migration job's status (requires migration scope and migration_job_get IAM permission)"
    )]
    pub(crate) async fn migration_job_status(
        &self,
        Parameters(args): Parameters<MigrationJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "migration_job_status");
        render(
            crate::models::queue_job_lifecycles::migration_job_status_for_actor(
                self.app_context(),
                actor,
                args.job_id,
            )
            .await,
        )
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Migration),
        description = "Enqueue workspace embedding reindex (requires migration scope and workspace_reindex IAM permission, rechecked before execution and publication)"
    )]
    pub(crate) async fn reindex_workspace(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "reindex_workspace");
        let job_id = try_tool!(
            crate::workers::reindex::enqueue_requested_for_actor(self.app_context(), actor).await
        );
        ok_json(crate::dtos::entities::ReindexResponse { job_id })
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Migration),
        description = "Enqueue undo of a migration job's snapshots (requires migration scope and migration_undo IAM permission, rechecked at execution)"
    )]
    pub(crate) async fn undo_migration_job(
        &self,
        Parameters(args): Parameters<MigrationJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts, "undo_migration_job");
        let job_id = try_tool!(
            crate::workers::undo::enqueue_for_actor(self.app_context(), actor, args.job_id).await
        );
        ok_json(crate::dtos::entities::UndoResponse {
            job_id,
            snapshot_job_id: args.job_id,
        })
    }
}

crate::dtos::common::list_params_from!(ListEntitiesArgs);
