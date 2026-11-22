//! REST routes for the workspace template library.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::dtos::template_library::{
    CreateTemplateRequest, ForkTemplateRequest, UpdateTemplateRequest,
};
use crate::models::template_templates::{
    self, CreateTemplateInput, TemplateRecord, UpdateTemplateInput,
};

#[cfg(feature = "openapi")]
fn list_templates_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/template-library",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-actions" :  serde_json::json!(["template_library_list"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<TemplateRecord>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "page", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}, {"name": "page_size", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}]}),
    )
}
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn list_templates(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<TemplateRecord>>, ApiError> {
    let templates = template_templates::list_as(&ctx.db, &auth, page.try_into()?).await?;
    Ok(Json(templates))
}

#[cfg(feature = "openapi")]
fn get_template_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/template-library/{id}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-actions" :  serde_json::json!(["template_library_get"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<TemplateRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn get_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<TemplateRecord>, ApiError> {
    let template = template_templates::get_as(&ctx.db, &auth, id).await?;
    Ok(Json(template))
}

#[cfg(feature = "openapi")]
fn create_template_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/template-library",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<CreateTemplateRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-actions" :  serde_json::json!(["template_library_create"]), "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<TemplateRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn create_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<CreateTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let template = template_templates::create_as(
        &ctx.db,
        &auth,
        CreateTemplateInput {
            name: body.name,
            description: body.description,
            definition: body.definition,
            tags: body.tags,
            locale: body.locale,
            author: body.author,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(template)))
}

#[cfg(feature = "openapi")]
fn update_template_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/template-library/{id}",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<UpdateTemplateRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-actions" :  serde_json::json!(["template_library_update"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<TemplateRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn update_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTemplateRequest>,
) -> Result<Json<TemplateRecord>, ApiError> {
    let template = template_templates::update_as(
        &ctx.db,
        &auth,
        id,
        UpdateTemplateInput {
            name: body.name,
            description: body.description,
            definition: body.definition,
            tags: body.tags,
            locale: body.locale,
        },
    )
    .await?;
    Ok(Json(template))
}

#[cfg(feature = "openapi")]
fn delete_template_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/template-library/{id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-actions" :  serde_json::json!(["template_library_delete"]), "responses": {"204": serde_json::json!({"description": "Template deleted"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
pub(crate) async fn delete_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    template_templates::delete_as(&ctx.db, &auth, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
fn fork_template_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/template-library/{id}/fork",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<ForkTemplateRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-actions" :  serde_json::json!(["template_library_fork"]), "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<TemplateRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn fork_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
    Json(body): Json<ForkTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let template = template_templates::fork_as(&ctx.db, &auth, id, body.name).await?;
    Ok((StatusCode::CREATED, Json(template)))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![
        list_templates_doc(),
        get_template_doc(),
        create_template_doc(),
        update_template_doc(),
        delete_template_doc(),
        fork_template_doc(),
    ]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/template-library")
        .add("/", get(list_templates))
        .add("/", post(create_template))
        .add("/{id}", get(get_template))
        .add("/{id}", put(update_template))
        .add("/{id}", axum::routing::delete(delete_template))
        .add("/{id}/fork", post(fork_template))
}
