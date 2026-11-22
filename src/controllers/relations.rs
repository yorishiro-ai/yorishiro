//! REST routes for entity relations.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{AuthContext, Authorized, ReadScope, Verified, WriteScope};
use crate::dtos::relations::{
    CreateRelationRequest, ListRelationsParams, SetRelationStatusRequest,
};
use crate::models::entity_relations::{self, RelationRecord};

#[cfg(feature = "openapi")]
fn create_relation_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/relations",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<CreateRelationRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["write"]), "x-yorishiro-required-actions" :  serde_json::json!(["relation_create"]), "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<RelationRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
pub(crate) async fn create_relation(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Json(body): Json<CreateRelationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        crate::models::entity_entities::workspace_for_actor(&ctx.db, &verified.ctx, body.source_id)
            .await?,
    );
    let mut authorized = Authorized::<WriteScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::RelationCreate)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    let input = body.into();
    let record = entity_relations::create(authorized.txn(), workspace_id, input).await?;
    authorized.commit().await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[cfg(feature = "openapi")]
fn get_relation_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/relations/{id}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["relation_get"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<RelationRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
pub(crate) async fn get_relation(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(id): Path<Uuid>,
) -> Result<Json<RelationRecord>, ApiError> {
    let mut authorized = Authorized::<ReadScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Relation(id),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::RelationGet)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    let record = entity_relations::get(authorized.txn(), workspace_id, id).await?;
    Ok(Json(record))
}

#[cfg(feature = "openapi")]
fn delete_relation_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/relations/{id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["write"]), "x-yorishiro-required-actions" :  serde_json::json!(["relation_delete"]), "responses": {"204": serde_json::json!({"description": "Relation deleted"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
pub(crate) async fn delete_relation(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let mut authorized = Authorized::<WriteScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Relation(id),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::RelationDelete)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    entity_relations::delete(authorized.txn(), workspace_id, id).await?;
    authorized.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
fn list_relations_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/relations",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["relation_list"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<RelationRecord>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "source_id", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}, {"name": "target_id", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}, {"name": "relation_type", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}, {"name": "status", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<entity_relations::RelationStatus>()}, {"name": "page", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}, {"name": "page_size", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}]}),
    )
}
pub(crate) async fn list_relations(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Query(params): Query<ListRelationsParams>,
) -> Result<Json<Vec<RelationRecord>>, ApiError> {
    let query = params.try_into()?;

    let records = entity_relations::list_for_actor(&ctx, &actor, query).await?;
    Ok(Json(records))
}

#[cfg(feature = "openapi")]
fn set_relation_status_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/relations/{id}/status",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<SetRelationStatusRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["write"]), "x-yorishiro-required-actions" :  serde_json::json!(["relation_update"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<RelationRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
pub(crate) async fn set_relation_status(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetRelationStatusRequest>,
) -> Result<Json<RelationRecord>, ApiError> {
    let mut authorized = Authorized::<WriteScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Relation(id),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::RelationUpdate)
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
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![
        create_relation_doc(),
        get_relation_doc(),
        delete_relation_doc(),
        list_relations_doc(),
        set_relation_status_doc(),
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
