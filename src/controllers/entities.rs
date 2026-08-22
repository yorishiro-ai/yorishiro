use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use loco_rs::controller::Routes;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, WriteScope};
use crate::models::content_entities::{self, EntityRecord};

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
    // Embedding sync runs asynchronously off the request path in the old implementation; the
    // worker port for it hasn't landed yet in this rebuild, so it's a no-op for now.
    authorized.commit().await?;
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
    authorized: Authorized<WriteScope>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateEntityRequest>,
) -> Result<Json<EntityRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let updated_by = authorized.ctx.user_id;
    let record =
        content_entities::update(authorized.txn(), workspace_id, id, body.data, updated_by).await?;
    // Embedding sync runs asynchronously off the request path in the old implementation; the
    // worker port for it hasn't landed yet in this rebuild, so it's a no-op for now.
    authorized.commit().await?;
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

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/entities")
        .add("/", post(create_entity))
        .add("/", get(list_entities))
        .add("/{id}", get(get_entity))
        .add("/{id}", put(update_entity))
        .add("/{id}", delete(delete_entity))
}
