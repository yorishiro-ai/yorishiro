use axum::Json;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use yorishiro_core::metaschema::MetaSchemaDefinition;
use yorishiro_core::repositories::workspace_schemas::{self, WorkspaceSchemaRecord};

use crate::error::ApiError;
use crate::http::middleware::auth::{Authorized, ReadScope, SchemaScope};

#[derive(Debug, Deserialize, ToSchema)]
pub struct ForkSchemaRequest {
    /// The tenant schema to fork. Its currently active version is the one copied.
    pub schema_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceSchemaStatus {
    /// `null` when this workspace has not forked and uses its tenant's schema directly.
    pub fork: Option<WorkspaceSchemaRecord>,
    /// The tenant schema's active version, when it is newer than the one this fork was taken
    /// from. `null` means there is nothing to follow.
    pub upstream_version: Option<i32>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct FollowUpstreamRequest {
    /// Required to overwrite a fork that has been edited. Without it, a customized fork is
    /// refused rather than having its edits discarded.
    #[serde(default)]
    pub force: bool,
}

#[utoipa::path(
    get,
    path = "/api/workspace-schema",
    responses(
        (status = 200, description = "This workspace's schema fork, if it has one", body = WorkspaceSchemaStatus),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
    ),
    tag = "schemas",
)]
pub async fn get_workspace_schema(
    mut authorized: Authorized<ReadScope>,
) -> Result<Json<WorkspaceSchemaStatus>, ApiError> {
    let tenant_id = authorized.ctx.tenant_id;
    let workspace_id = authorized.ctx.workspace_id;

    let fork = workspace_schemas::get(authorized.conn(), workspace_id).await?;
    let upstream_version = match &fork {
        Some(fork) => {
            workspace_schemas::upstream_version(authorized.conn(), tenant_id, fork).await?
        }
        None => None,
    };

    Ok(Json(WorkspaceSchemaStatus {
        fork,
        upstream_version,
    }))
}

#[utoipa::path(
    post,
    path = "/api/workspace-schema",
    request_body = ForkSchemaRequest,
    responses(
        (status = 201, description = "Schema forked into this workspace", body = WorkspaceSchemaRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "No active schema exists with the given name", body = crate::error::ApiErrorBody),
        (status = 409, description = "This workspace has already forked its schema", body = crate::error::ApiErrorBody),
    ),
    tag = "schemas",
)]
pub async fn fork_schema(
    mut authorized: Authorized<SchemaScope>,
    Json(body): Json<ForkSchemaRequest>,
) -> Result<(StatusCode, Json<WorkspaceSchemaRecord>), ApiError> {
    let tenant_id = authorized.ctx.tenant_id;
    let workspace_id = authorized.ctx.workspace_id;
    let record = workspace_schemas::fork(
        authorized.conn(),
        tenant_id,
        workspace_id,
        &body.schema_name,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    put,
    path = "/api/workspace-schema",
    request_body = MetaSchemaDefinition,
    responses(
        (status = 200, description = "This workspace's fork updated", body = WorkspaceSchemaRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "This workspace has not forked its schema", body = crate::error::ApiErrorBody),
        (status = 422, description = "The definition is not a valid metaschema", body = crate::error::ApiErrorBody),
    ),
    tag = "schemas",
)]
pub async fn update_workspace_schema(
    mut authorized: Authorized<SchemaScope>,
    Json(definition): Json<MetaSchemaDefinition>,
) -> Result<Json<WorkspaceSchemaRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record =
        workspace_schemas::update_definition(authorized.conn(), workspace_id, definition).await?;
    Ok(Json(record))
}

#[utoipa::path(
    post,
    path = "/api/workspace-schema/follow",
    request_body = FollowUpstreamRequest,
    responses(
        (status = 200, description = "This workspace's fork replaced with the tenant's active schema", body = WorkspaceSchemaRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "This workspace has not forked its schema", body = crate::error::ApiErrorBody),
        (status = 409, description = "The fork has local edits and `force` was not set", body = crate::error::ApiErrorBody),
    ),
    tag = "schemas",
)]
pub async fn follow_upstream(
    mut authorized: Authorized<SchemaScope>,
    Json(body): Json<FollowUpstreamRequest>,
) -> Result<Json<WorkspaceSchemaRecord>, ApiError> {
    let tenant_id = authorized.ctx.tenant_id;
    let workspace_id = authorized.ctx.workspace_id;
    let record =
        workspace_schemas::follow_upstream(authorized.conn(), tenant_id, workspace_id, body.force)
            .await?;
    Ok(Json(record))
}

#[utoipa::path(
    delete,
    path = "/api/workspace-schema",
    responses(
        (status = 204, description = "Fork dropped; this workspace uses its tenant's schema again"),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "This workspace has not forked its schema", body = crate::error::ApiErrorBody),
    ),
    tag = "schemas",
)]
pub async fn unfork_schema(
    mut authorized: Authorized<SchemaScope>,
) -> Result<StatusCode, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    workspace_schemas::unfork(authorized.conn(), workspace_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[path = "../../../tests/http/controllers/workspace_schemas.rs"]
mod tests;
