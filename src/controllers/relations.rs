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
use crate::models::content_relations::{self, RelationRecord};

#[derive(Debug, Deserialize)]
pub struct CreateRelationRequest {
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    pub properties: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct ListRelationsParams {
    pub source_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub relation_type: Option<String>,
    /// Restricts the listing to one state.
    /// Omitted, every state is listed.
    pub status: Option<String>,
    #[serde(flatten)]
    pub page: crate::controllers::PageParams,
}

#[derive(Debug, Deserialize)]
pub struct SetRelationStatusRequest {
    /// `active`, `deprecated` or `archived`.
    pub status: String,
}

pub async fn create_relation(
    authorized: Authorized<WriteScope>,
    Json(body): Json<CreateRelationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let input = content_relations::CreateRelationInput {
        source_id: body.source_id,
        target_id: body.target_id,
        relation_type: body.relation_type,
        properties: body.properties.unwrap_or_else(|| serde_json::json!({})),
    };
    let record = content_relations::create(authorized.txn(), workspace_id, input).await?;
    authorized.commit().await?;
    Ok((StatusCode::CREATED, Json(record)))
}

pub async fn get_relation(
    authorized: Authorized<ReadScope>,
    Path(id): Path<Uuid>,
) -> Result<Json<RelationRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record = content_relations::get(authorized.txn(), workspace_id, id).await?;
    Ok(Json(record))
}

pub async fn delete_relation(
    authorized: Authorized<WriteScope>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    content_relations::delete(authorized.txn(), workspace_id, id).await?;
    authorized.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_relations(
    authorized: Authorized<ReadScope>,
    Query(params): Query<ListRelationsParams>,
) -> Result<Json<Vec<RelationRecord>>, ApiError> {
    let query = content_relations::ListRelationsQuery {
        source_id: params.source_id,
        target_id: params.target_id,
        relation_type: params.relation_type,
        status: params.status,
        page: params.page.into(),
    };

    let workspace_id = authorized.ctx.workspace_id;
    let records = content_relations::list(authorized.txn(), workspace_id, query).await?;
    Ok(Json(records))
}

pub async fn set_relation_status(
    authorized: Authorized<WriteScope>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetRelationStatusRequest>,
) -> Result<Json<RelationRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record =
        content_relations::set_status(authorized.txn(), workspace_id, id, &body.status).await?;
    authorized.commit().await?;
    Ok(Json(record))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/relations")
        .add("/", post(create_relation))
        .add("/", get(list_relations))
        .add("/{id}", get(get_relation))
        .add("/{id}", delete(delete_relation))
        .add("/{id}/status", put(set_relation_status))
}
