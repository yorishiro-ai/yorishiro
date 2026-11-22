//! REST endpoints for workspace-owned schema forks.

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, SchemaScope, Verified};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use uuid::Uuid;

use crate::edition::ee::models::workspace_schema_forks as model;

#[cfg(feature = "openapi")]
fn list_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schema-forks",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_fork_list"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<crate::edition::ee::controllers::openapi::ForkRecord>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn list(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<model::ForkRecord>>, ApiError> {
    Ok(Json(
        model::list_for_actor(&ctx, &verified.ctx, page.try_into()?).await?,
    ))
}

#[cfg(feature = "openapi")]
fn get_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schema-forks/{fork_id}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_fork_get"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::edition::ee::controllers::openapi::ForkRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "fork_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
async fn get(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(fork_id): Path<Uuid>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        model::workspace_for_actor(&ctx.db, &verified.ctx, fork_id).await?,
    );
    let mut authorized = Authorized::<ReadScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaForkGet)
        .await?;
    Ok(Json(
        model::get(
            authorized.txn(),
            authorized.ctx.tenant_id,
            authorized.ctx.require_workspace()?,
            fork_id,
        )
        .await?,
    ))
}

#[cfg(feature = "openapi")]
fn create_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schema-forks",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["enterprise"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::edition::ee::controllers::openapi::CreateForkRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_fork_create"]), "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::edition::ee::controllers::openapi::ForkRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn create(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Json(input): Json<model::CreateInput>,
) -> Result<(StatusCode, Json<model::ForkRecord>), ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Permission(
        crate::models::iam::policy::Action::SchemaForkCreate,
    );
    let mut authorized = Authorized::<SchemaScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaForkCreate)
        .await?;
    let fork = model::create(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.require_workspace()?,
        input,
    )
    .await?;
    authorized.commit().await?;
    Ok((StatusCode::CREATED, Json(fork)))
}

#[cfg(feature = "openapi")]
fn update_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schema-forks/{fork_id}",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["enterprise"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::edition::ee::controllers::openapi::UpdateForkRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_fork_update"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::edition::ee::controllers::openapi::ForkRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "fork_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
async fn update(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(fork_id): Path<Uuid>,
    Json(input): Json<model::UpdateInput>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        model::workspace_for_actor(&ctx.db, &verified.ctx, fork_id).await?,
    );
    let mut authorized = Authorized::<SchemaScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaForkUpdate)
        .await?;
    let fork = model::update(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.require_workspace()?,
        fork_id,
        input,
    )
    .await?;
    authorized.commit().await?;
    Ok(Json(fork))
}

#[cfg(feature = "openapi")]
fn delete_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schema-forks/{fork_id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_fork_delete"]), "responses": {"204": serde_json::json!({"description": "Schema fork deleted"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "fork_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
async fn delete(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(fork_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        model::workspace_for_actor(&ctx.db, &verified.ctx, fork_id).await?,
    );
    let mut authorized = Authorized::<SchemaScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaForkDelete)
        .await?;
    model::delete(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.require_workspace()?,
        fork_id,
    )
    .await?;
    authorized.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::openapi::route_inventory::RouteDoc> {
    vec![
        list_doc(),
        create_doc(),
        get_doc(),
        update_doc(),
        delete_doc(),
    ]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/schema-forks")
        .add("", axum::routing::get(list).post(create))
        .add(
            "/{fork_id}",
            axum::routing::get(get).put(update).delete(delete),
        )
}
