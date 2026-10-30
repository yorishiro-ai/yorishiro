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

use super::{AuthzOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::models::api_keys::ApiKeyScope;
use crate::models::entity_entities;

#[derive(Deserialize, JsonSchema)]
pub struct CreateEntityArgs {
    /// Name of the schema this entity conforms to.
    /// The workspace's current active version is used.
    pub schema_name: String,
    /// entity_type name declared in the schema.
    pub entity_type: String,
    /// Entity body (JSON object) conforming to the schema's `fields` definition.
    pub data: Value,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct GetEntityArgs {
    pub id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct UpdateEntityArgs {
    pub id: Uuid,
    /// Replacement entity body.
    /// Validated against the schema version in effect when the entity was created.
    pub data: Value,
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
pub(crate) struct FillDefaultsArgs {
    /// Name of the schema whose active version supplies the missing fields.
    pub schema_name: String,
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

impl From<CreateEntityArgs> for entity_entities::CreateEntityInput {
    fn from(args: CreateEntityArgs) -> Self {
        Self {
            schema_name: args.schema_name,
            entity_type: args.entity_type,
            data: args.data,
        }
    }
}

impl TryFrom<ListEntitiesArgs> for entity_entities::ListEntitiesQuery {
    type Error = crate::YorishiroError;

    fn try_from(args: ListEntitiesArgs) -> Result<Self, Self::Error> {
        Ok(Self {
            entity_type: args.entity_type,
            filter: args.filter,
            schema_version: args.schema_version,
            page: crate::models::pagination::ListParams::new(args.limit, args.offset),
        })
    }
}

#[tool_router(vis = "pub(crate)", router = tool_router_entities)]
impl YorishiroMcpServer {
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write),description = "Create a new entity (requires write scope)")]
    pub async fn create_entity(
        &self,
        Parameters(args): Parameters<CreateEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("create_entity")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let input = args.into();

        let workspace_id = authorized.auth_context().workspace_id;
        let created_by = authorized.auth_context().user_id;
        let quotas = match crate::quotas(
            self.app_context(),
            authorized.txn(),
            authorized.auth_context().tenant_id,
        )
        .await
        {
            Ok(quotas) => quotas,
            Err(error) => return Ok(err_to_tool_result(error)),
        };
        let record = match entity_entities::create_with_quota(
            authorized.txn(),
            workspace_id,
            input,
            created_by,
            quotas.entities,
        )
        .await
        {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        // Same durable enqueue as REST, committed atomically with the entity.
        if let Err(err) = crate::workers::embedding_sync::enqueue_after_write(
            self.app_context(),
            workspace_id,
            record.id,
            authorized.txn(),
        )
        .await
        {
            return Ok(err_to_tool_result(err));
        }
        authorized.commit().await?;
        ok_json(record)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),description = "Get a single entity by ID (requires read scope)")]
    pub async fn get_entity(
        &self,
        Parameters(args): Parameters<GetEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("get_entity")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let workspace_id = authorized.auth_context().workspace_id;
        let record = match entity_entities::get(authorized.txn(), workspace_id, args.id).await {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        ok_json(record)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write),description = "Replace the data of an existing entity (requires write scope)")]
    pub(crate) async fn update_entity(
        &self,
        Parameters(args): Parameters<UpdateEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("update_entity")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let workspace_id = authorized.auth_context().workspace_id;
        let updated_by = authorized.auth_context().user_id;
        let record = match entity_entities::update(
            authorized.txn(),
            workspace_id,
            entity_entities::UpdateEntityInput {
                id: args.id,
                data: args.data,
                updated_by,
            },
        )
        .await
        {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        if let Err(err) = crate::workers::embedding_sync::enqueue_after_write(
            self.app_context(),
            workspace_id,
            record.id,
            authorized.txn(),
        )
        .await
        {
            return Ok(err_to_tool_result(err));
        }
        authorized.commit().await?;
        ok_json(record)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Write),description = "Delete an entity (requires write scope)")]
    pub(crate) async fn delete_entity(
        &self,
        Parameters(args): Parameters<DeleteEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("delete_entity")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let workspace_id = authorized.auth_context().workspace_id;
        match entity_entities::delete(authorized.txn(), workspace_id, args.id).await {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        authorized.commit().await?;
        ok_json(serde_json::json!({ "deleted": true }))
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),description = "List entities (requires read scope)")]
    pub async fn list_entities(
        &self,
        Parameters(args): Parameters<ListEntitiesArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("list_entities")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let query = match args.try_into() {
            Ok(query) => query,
            Err(err) => return Ok(err_to_tool_result(err)),
        };

        let workspace_id = authorized.auth_context().workspace_id;
        let records = match entity_entities::list(authorized.txn(), workspace_id, query).await {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        ok_json(records)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Report how an entity stands against the active version of its schema \
                           (requires read scope). Entities are migrated lazily, so one written \
                           against an older version can still contain newer fields. Reports active \
                           fields absent from stored data, including nested fields in existing \
                           objects. Present nulls and type errors are not missing-value diagnostics."
    )]
    pub(crate) async fn get_entity_drift(
        &self,
        Parameters(args): Parameters<GetEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("get_entity_drift")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let workspace_id = authorized.auth_context().workspace_id;
        let drift = match entity_entities::drift(authorized.txn(), workspace_id, args.id).await {
            Ok(value) => value,
            Err(err) => return Ok(err_to_tool_result(err)),
        };
        ok_json(drift)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Count what migrating a schema's entities to its active version would \
                           face, without doing it (requires read scope). Reports how many are \
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
        let authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("migration_dry_run")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let workspace_id = authorized.auth_context().workspace_id;
        let report =
            match entity_entities::migration_dry_run(authorized.txn(), workspace_id, &args.name)
                .await
            {
                Ok(value) => value,
                Err(err) => return Ok(err_to_tool_result(err)),
            };
        ok_json(report)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Migration),
        description = "Enqueue a fill-defaults job (requires migration scope). Returns a job ID; the worker fills absent required fields and takes undo snapshots. Completed counts appear in the audit log."
    )]
    pub async fn fill_defaults(
        &self,
        Parameters(args): Parameters<FillDefaultsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("fill_defaults")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };

        let job_id = match crate::workers::fill_defaults::enqueue(
            self.app_context(),
            authorized.txn(),
            authorized.auth_context(),
            args.schema_name.clone(),
        )
        .await
        {
            Ok(id) => id,
            Err(error) => return Ok(err_to_tool_result(error)),
        };
        authorized.commit().await?;
        ok_json(crate::dtos::entities::FillDefaultsResponse {
            schema_name: args.schema_name,
            job_id,
        })
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Migration),
        description = "Get a migration job's status (requires migration scope)"
    )]
    pub(crate) async fn migration_job_status(
        &self,
        Parameters(args): Parameters<MigrationJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("migration_job_status")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::queue_job_lifecycles::migration_job_status(
            authorized.txn(),
            authorized.auth_context().workspace_id,
            args.job_id,
        )
        .await
        {
            Ok(status) => ok_json(status),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Migration),
        description = "Enqueue undo of a migration job's snapshots (requires migration scope)"
    )]
    pub(crate) async fn undo_migration_job(
        &self,
        Parameters(args): Parameters<MigrationJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let authorized = match super::authorize(
            self.app_context(),
            &parts,
            self.required_scope("undo_migration_job")?,
        )
        .await?
        {
            AuthzOutcome::Authorized(authorized) => authorized,
            AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        let job_id = match crate::workers::undo::enqueue(
            self.app_context(),
            authorized.txn(),
            authorized.auth_context(),
            args.job_id,
        )
        .await
        {
            Ok(id) => id,
            Err(error) => return Ok(err_to_tool_result(error)),
        };
        authorized.commit().await?;
        ok_json(crate::dtos::entities::UndoResponse {
            job_id,
            snapshot_job_id: args.job_id,
        })
    }
}
