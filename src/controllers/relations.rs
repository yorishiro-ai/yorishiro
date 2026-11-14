use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use loco_rs::controller::Routes;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, WriteScope};
use crate::dtos::relations::{
    CreateRelationRequest, ListRelationsParams, SetRelationStatusRequest,
};
use crate::models::entity_relations::{self, RelationRecord};

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/relations", request_body = crate::dtos::relations::CreateRelationRequest, responses((status = 201, body = crate::models::entity_relations::RelationRecord), (status = 401, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["write"])), ("x-yorishiro-required-actions" = json!(["relation_create"]))), tag = "community"))]
pub(crate) async fn create_relation(
    mut authorized: Authorized<WriteScope>,
    Json(body): Json<CreateRelationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    authorized
        .require_action(crate::models::iam_policy::Action::RelationCreate)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    let input = body.into();
    let record = entity_relations::create(authorized.txn(), workspace_id, input).await?;
    authorized.commit().await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/relations/{id}", params(("id" = Uuid, Path)), responses((status = 200, body = crate::models::entity_relations::RelationRecord), (status = 401, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["relation_get"]))), tag = "community"))]
pub(crate) async fn get_relation(
    mut authorized: Authorized<ReadScope>,
    Path(id): Path<Uuid>,
) -> Result<Json<RelationRecord>, ApiError> {
    authorized
        .require_action(crate::models::iam_policy::Action::RelationGet)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    let record = entity_relations::get(authorized.txn(), workspace_id, id).await?;
    Ok(Json(record))
}

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/relations/{id}", params(("id" = Uuid, Path)), responses((status = 204, description = "Relation deleted"), (status = 401, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["write"])), ("x-yorishiro-required-actions" = json!(["relation_delete"]))), tag = "community"))]
pub(crate) async fn delete_relation(
    mut authorized: Authorized<WriteScope>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    authorized
        .require_action(crate::models::iam_policy::Action::RelationDelete)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    entity_relations::delete(authorized.txn(), workspace_id, id).await?;
    authorized.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/relations", params(("source_id" = Option<Uuid>, Query), ("target_id" = Option<Uuid>, Query), ("relation_type" = Option<String>, Query), ("status" = Option<crate::models::entity_relations::RelationStatus>, Query), ("page" = Option<i32>, Query), ("page_size" = Option<i32>, Query)), responses((status = 200, body = [crate::models::entity_relations::RelationRecord]), (status = 401, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["relation_list"]))), tag = "community"))]
pub(crate) async fn list_relations(
    mut authorized: Authorized<ReadScope>,
    Query(params): Query<ListRelationsParams>,
) -> Result<Json<Vec<RelationRecord>>, ApiError> {
    authorized
        .require_action(crate::models::iam_policy::Action::RelationList)
        .await?;
    let query = params.try_into()?;

    let workspace_id = authorized.ctx.require_workspace()?;
    let records = entity_relations::list(authorized.txn(), workspace_id, query).await?;
    Ok(Json(records))
}

#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/relations/{id}/status", params(("id" = Uuid, Path)), request_body = crate::dtos::relations::SetRelationStatusRequest, responses((status = 200, body = crate::models::entity_relations::RelationRecord), (status = 401, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["write"])), ("x-yorishiro-required-actions" = json!(["relation_update"]))), tag = "community"))]
pub(crate) async fn set_relation_status(
    mut authorized: Authorized<WriteScope>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetRelationStatusRequest>,
) -> Result<Json<RelationRecord>, ApiError> {
    authorized
        .require_action(crate::models::iam_policy::Action::RelationUpdate)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    let record = entity_relations::set_status(
        authorized.txn(),
        workspace_id,
        entity_relations::SetRelationStatusInput {
            id,
            status: body.status,
        },
    )
    .await?;
    authorized.commit().await?;
    Ok(Json(record))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_create_relation),
        super::route_inventory::path_doc(__path_get_relation),
        super::route_inventory::path_doc(__path_delete_relation),
        super::route_inventory::path_doc(__path_list_relations),
        super::route_inventory::path_doc(__path_set_relation_status),
    ]
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
