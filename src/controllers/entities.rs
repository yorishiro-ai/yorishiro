use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
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

pub async fn create_entity(
    mut authorized: Authorized<WriteScope>,
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
        content_entities::create(authorized.conn(), workspace_id, input, created_by).await?;
    // Embedding sync runs asynchronously off the request path in the old implementation; the
    // worker port for it hasn't landed yet in this rebuild, so it's a no-op for now.
    Ok((StatusCode::CREATED, Json(record)))
}

pub async fn get_entity(
    mut authorized: Authorized<ReadScope>,
    Path(id): Path<Uuid>,
) -> Result<Json<EntityRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record = content_entities::get(authorized.conn(), workspace_id, id).await?;
    Ok(Json(record))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/entities")
        .add("/", post(create_entity))
        .add("/{id}", get(get_entity))
}
