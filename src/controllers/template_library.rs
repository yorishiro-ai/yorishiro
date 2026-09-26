use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::TransactionTrait;
use serde::Deserialize;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::members::require_tenant_admin;
use crate::error::YorishiroError;
use crate::metaschema::MetaSchemaDefinition;
use crate::models::template_templates::{
    self, CreateTemplateInput, TemplateRecord, UpdateTemplateInput,
};

#[utoipa::path(get, path = "/api/template-library", params(("page" = Option<i32>, Query), ("page_size" = Option<i32>, Query)), responses((status = 200, body = [super::openapi::TemplateRecord]), (status = 401, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), tag = "community")]
pub async fn list_templates(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Query(page): Query<crate::controllers::PageParams>,
) -> Result<Json<Vec<TemplateRecord>>, ApiError> {
    let templates =
        template_templates::list_templates(&ctx.db, auth.tenant_id, page.into()).await?;
    Ok(Json(templates))
}

#[utoipa::path(get, path = "/api/template-library/{id}", params(("id" = Uuid, Path)), responses((status = 200, body = super::openapi::TemplateRecord), (status = 401, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), tag = "community")]
pub async fn get_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<TemplateRecord>, ApiError> {
    let template = template_templates::get_template(&ctx.db, auth.tenant_id, id).await?;
    Ok(Json(template))
}

#[derive(Deserialize)]
pub struct CreateTemplateRequest {
    pub name: String,
    pub description: Option<String>,
    pub definition: MetaSchemaDefinition,
    #[serde(default)]
    pub tags: Vec<String>,
    pub locale: Option<String>,
    pub author: Option<String>,
}

#[utoipa::path(post, path = "/api/template-library", request_body = super::openapi::CreateTemplateRequest, responses((status = 201, body = super::openapi::TemplateRecord), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community")]
pub async fn create_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<CreateTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;

    let template = template_templates::create_template(
        &ctx.db,
        auth.tenant_id,
        auth.user_id,
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

#[derive(Deserialize)]
pub struct UpdateTemplateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub definition: Option<MetaSchemaDefinition>,
    pub tags: Option<Vec<String>>,
    pub locale: Option<String>,
}

#[utoipa::path(put, path = "/api/template-library/{id}", params(("id" = Uuid, Path)), request_body = super::openapi::UpdateTemplateRequest, responses((status = 200, body = super::openapi::TemplateRecord), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community")]
pub async fn update_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTemplateRequest>,
) -> Result<Json<TemplateRecord>, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;

    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|err| ApiError(YorishiroError::Internal(anyhow::anyhow!(err))))?;
    let template = template_templates::update_template(
        &txn,
        auth.tenant_id,
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
    txn.commit()
        .await
        .map_err(|err| ApiError(YorishiroError::Internal(anyhow::anyhow!(err))))?;
    Ok(Json(template))
}

#[utoipa::path(delete, path = "/api/template-library/{id}", params(("id" = Uuid, Path)), responses((status = 204, description = "Template deleted"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community")]
pub async fn delete_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;
    template_templates::delete_template(&ctx.db, auth.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct ForkTemplateRequest {
    pub name: String,
}

#[utoipa::path(post, path = "/api/template-library/{id}/fork", params(("id" = Uuid, Path)), request_body = super::openapi::ForkTemplateRequest, responses((status = 201, body = super::openapi::TemplateRecord), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community")]
pub async fn fork_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
    Json(body): Json<ForkTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;

    let template =
        template_templates::fork_template(&ctx.db, auth.tenant_id, auth.user_id, id, body.name)
            .await?;
    Ok((StatusCode::CREATED, Json(template)))
}

pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_list_templates),
        super::route_inventory::path_doc(__path_get_template),
        super::route_inventory::path_doc(__path_create_template),
        super::route_inventory::path_doc(__path_update_template),
        super::route_inventory::path_doc(__path_delete_template),
        super::route_inventory::path_doc(__path_fork_template),
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
