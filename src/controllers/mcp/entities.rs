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
use crate::models::entity_entities;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct GetEntityArgs {
    pub entity_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct UpdateEntityArgs {
    pub entity_id: Uuid,
    #[serde(flatten)]
    pub body: UpdateEntityRequest,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct DeleteEntityArgs {
    pub entity_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct MigrationDryRunArgs {
    pub schema_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct MigrationJobArgs {
    pub migration_job_id: Uuid,
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
    #[tool(description = "Create an entity.")]
    pub async fn create_entity(
        &self,
        Parameters(args): Parameters<CreateEntityRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let record = try_tool!(
            entity_entities::create_for_actor(self.app_context(), &actor, args.into(),).await
        );
        ok_json(record)
    }

    #[tool(description = "Get an entity by ID.")]
    pub async fn get_entity(
        &self,
        Parameters(args): Parameters<GetEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let record = try_tool!(
            entity_entities::get_for_actor(self.app_context(), &actor, args.entity_id).await
        );
        ok_json(record)
    }

    #[tool(
        description = "Replace an entity's data. Pass expected_updated_at from a prior read to prevent overwriting a concurrent update."
    )]
    pub(crate) async fn update_entity(
        &self,
        Parameters(args): Parameters<UpdateEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let updated_by = actor.user_id;
        let record = try_tool!(
            entity_entities::update_for_actor(
                self.app_context(),
                &actor,
                entity_entities::UpdateEntityInput {
                    id: args.entity_id,
                    data: args.body.data,
                    updated_by,
                    expected_updated_at: args.body.expected_updated_at,
                },
            )
            .await
        );
        ok_json(record)
    }

    #[tool(description = "Delete an entity.")]
    pub(crate) async fn delete_entity(
        &self,
        Parameters(args): Parameters<DeleteEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        try_tool!(
            entity_entities::delete_for_actor(self.app_context(), &actor, args.entity_id).await
        );
        ok_json(serde_json::json!({ "deleted": true }))
    }

    #[tool(description = "List entities.")]
    pub async fn list_entities(
        &self,
        Parameters(args): Parameters<ListEntitiesArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);

        let query = try_tool!(args.try_into());

        let records =
            try_tool!(entity_entities::list_for_actor(self.app_context(), &actor, query).await);
        ok_json(records)
    }

    #[tool(
        description = "Compare an entity with the active schema and report missing fields, including fields in nested objects. Nulls and type errors do not count as missing."
    )]
    pub(crate) async fn get_entity_drift(
        &self,
        Parameters(args): Parameters<GetEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(entity_entities::drift_for_actor(self.app_context(), actor, args.entity_id).await)
    }

    #[tool(
        description = "Count what migrating a schema's entities to its active version would \
                           face, without doing it. Reports how many are \
                           current, how many are behind without missing required fields, and how many lack a \
                           field the active version requires: the last being the work a \
                           migration would have to fill in. Inspects stored data, not only schema \
                           definitions; this is not full active-schema validation."
    )]
    pub async fn preview_migration(
        &self,
        Parameters(args): Parameters<MigrationDryRunArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            entity_entities::migration_dry_run_for_actor(
                self.app_context(),
                actor,
                &args.schema_name,
            )
            .await,
        )
    }

    #[tool(
        description = "Enqueue a job to fill absent required fields and take undo snapshots. Completed counts appear in the audit log."
    )]
    pub async fn fill_entity_defaults(
        &self,
        Parameters(args): Parameters<FillDefaultsRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
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
            migration_job_id: job_id,
        })
    }

    #[tool(description = "Get a migration job's status")]
    pub(crate) async fn get_migration_job(
        &self,
        Parameters(args): Parameters<MigrationJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            crate::models::queue_job_lifecycles::migration_job_status_for_actor(
                self.app_context(),
                actor,
                args.migration_job_id,
            )
            .await
            .map(|status| crate::dtos::jobs::MigrationJobStatus {
                migration_job_id: args.migration_job_id,
                status,
            }),
        )
    }

    #[tool(
        description = "Queue a rebuild of the workspace's embeddings. The worker rechecks authorization before execution and publication."
    )]
    pub(crate) async fn reindex_workspace(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let job_id = try_tool!(
            crate::workers::reindex::enqueue_requested_for_actor(self.app_context(), actor).await
        );
        ok_json(crate::dtos::entities::ReindexResponse {
            migration_job_id: job_id,
        })
    }

    #[tool(description = "Queue a job that restores entities from a migration job's snapshots.")]
    pub(crate) async fn undo_migration_job(
        &self,
        Parameters(args): Parameters<MigrationJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let job_id = try_tool!(
            crate::workers::undo::enqueue_for_actor(
                self.app_context(),
                actor,
                args.migration_job_id
            )
            .await
        );
        ok_json(crate::dtos::entities::UndoResponse {
            migration_job_id: job_id,
            snapshot_job_id: args.migration_job_id,
        })
    }
}

crate::dtos::common::list_params_from!(ListEntitiesArgs);
