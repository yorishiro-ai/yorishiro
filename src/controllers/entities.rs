use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{
    Authorized, MigrationScope, ReadScope, WriteScope, embedding_provider,
};
use crate::models::content_entities::{self, EntityRecord, UndoReport};

/// Fires embedding sync in the background after the caller's own transaction has committed:
/// generating a vector is an HTTP round trip to the embedding provider (up to 30s), and this
/// must never add that latency to the entity write it follows, nor hold a DB connection open for
/// it. Errors (provider unreachable, dimension mismatch) are logged, not surfaced: the entity
/// write already succeeded and embedding is an auxiliary feature (see `sync_embedding`'s doc
/// comment). Loco's queue backend isn't wired yet (`Hooks::connect_workers` is a no-op, see
/// #221's scope), so `tokio::spawn` is the lazy stand-in until it is.
fn spawn_embedding_sync(ctx: AppContext, workspace_id: Uuid, record: EntityRecord) {
    let Ok(provider) = embedding_provider(&ctx) else {
        return;
    };
    tokio::spawn(async move {
        if let Err(err) = crate::services::embedding::sync::sync_embedding_for_record(
            &ctx.db,
            workspace_id,
            &record,
            provider.as_ref(),
        )
        .await
        {
            tracing::warn!(entity_id = %record.id, error = %err, "embedding sync failed");
        }
    });
}

#[derive(Debug, Deserialize)]
pub struct CreateEntityRequest {
    pub schema_name: String,
    pub entity_type: String,
    pub data: Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateEntityRequest {
    pub data: Value,
}

#[derive(Debug, Deserialize)]
pub struct ListEntitiesParams {
    pub entity_type: Option<String>,
    /// JSON-encoded containment filter, e.g. `{"status":"active"}`.
    pub filter: Option<String>,
    /// Restricts results to entities created against this schema version.
    pub schema_version: Option<i32>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn create_entity(
    State(ctx): State<AppContext>,
    authorized: Authorized<WriteScope>,
    Json(body): Json<CreateEntityRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let input = content_entities::CreateEntityInput {
        schema_name: body.schema_name,
        entity_type: body.entity_type,
        data: body.data,
    };
    let created_by = authorized.ctx.user_id;
    let record =
        content_entities::create(authorized.txn(), workspace_id, input, created_by).await?;
    authorized.commit().await?;
    spawn_embedding_sync(ctx, workspace_id, record.clone());
    Ok((StatusCode::CREATED, Json(record)))
}

pub async fn get_entity(
    authorized: Authorized<ReadScope>,
    Path(id): Path<Uuid>,
) -> Result<Json<EntityRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record = content_entities::get(authorized.txn(), workspace_id, id).await?;
    Ok(Json(record))
}

pub async fn update_entity(
    State(ctx): State<AppContext>,
    authorized: Authorized<WriteScope>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateEntityRequest>,
) -> Result<Json<EntityRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let updated_by = authorized.ctx.user_id;
    let record =
        content_entities::update(authorized.txn(), workspace_id, id, body.data, updated_by).await?;
    authorized.commit().await?;
    spawn_embedding_sync(ctx, workspace_id, record.clone());
    Ok(Json(record))
}

pub async fn delete_entity(
    authorized: Authorized<WriteScope>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    content_entities::delete(authorized.txn(), workspace_id, id).await?;
    authorized.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_entities(
    authorized: Authorized<ReadScope>,
    Query(params): Query<ListEntitiesParams>,
) -> Result<Json<Vec<EntityRecord>>, ApiError> {
    let default = content_entities::ListEntitiesQuery::default();
    let query = content_entities::ListEntitiesQuery {
        entity_type: params.entity_type,
        filter: crate::controllers::parse_filter_param(params.filter)?,
        schema_version: params.schema_version,
        limit: params.limit.unwrap_or(default.limit),
        offset: params.offset.unwrap_or(default.offset),
    };

    let workspace_id = authorized.ctx.workspace_id;
    let records = content_entities::list(authorized.txn(), workspace_id, query).await?;
    Ok(Json(records))
}

/// Puts every entity a job's snapshots cover back to what it held before the job overwrote it.
///
/// `MigrationScope`, not `WriteScope`: undoing a batch is a migration operation (the same scope
/// that would gate the job that produced the snapshots, e.g. `ee/`'s fill-proposal confirmation),
/// not an ordinary entity write.
pub async fn undo_migration_job(
    authorized: Authorized<MigrationScope>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<UndoReport>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let report = content_entities::undo_job(authorized.txn(), workspace_id, job_id).await?;
    authorized.commit().await?;
    Ok(Json(report))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/entities")
        .add("/", post(create_entity))
        .add("/", get(list_entities))
        .add("/{id}", get(get_entity))
        .add("/{id}", put(update_entity))
        .add("/{id}", delete(delete_entity))
}

pub fn migration_routes() -> Routes {
    Routes::new()
        .prefix("api/migration-jobs")
        .add("/{job_id}/undo", post(undo_migration_job))
}
