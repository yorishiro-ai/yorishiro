use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;
use yorishiro_core::metaschema::MetaSchemaDefinition;
use yorishiro_core::models::tenancy::{
    self, CreateTemplateInput, TemplateRecord, UpdateTemplateInput,
};

use crate::error::ApiError;
use crate::http::controllers::require_tenant_admin;
use crate::http::middleware::auth::AuthContext;
use crate::state::AppState;

#[utoipa::path(
    get,
    path = "/api/template-library",
    operation_id = "list_template_library",
    responses(
        (status = 200, description = "Templates visible to the caller's tenant (own plus any community-visible ones)", body = Vec<TemplateRecord>),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
    ),
    tag = "template-library",
)]
pub async fn list_templates(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
) -> Result<Json<Vec<TemplateRecord>>, ApiError> {
    let templates = tenancy::list_templates(state.identity_pool()?, ctx.tenant_id).await?;
    Ok(Json(templates))
}

#[utoipa::path(
    get,
    path = "/api/template-library/{id}",
    operation_id = "get_template_library_item",
    params(("id" = Uuid, Path, description = "Template ID")),
    responses(
        (status = 200, description = "Template detail", body = TemplateRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 404, description = "Template not found", body = crate::error::ApiErrorBody),
    ),
    tag = "template-library",
)]
pub async fn get_template(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<TemplateRecord>, ApiError> {
    let template = tenancy::get_template(state.identity_pool()?, ctx.tenant_id, id).await?;
    Ok(Json(template))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTemplateRequest {
    pub name: String,
    pub description: Option<String>,
    pub definition: MetaSchemaDefinition,
    #[serde(default)]
    pub tags: Vec<String>,
    pub locale: Option<String>,
    pub author: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/template-library",
    request_body = CreateTemplateRequest,
    responses(
        (status = 201, description = "Template created", body = TemplateRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Not a tenant owner/admin", body = crate::error::ApiErrorBody),
        (status = 409, description = "A template with this name already exists for the tenant", body = crate::error::ApiErrorBody),
        (status = 422, description = "The template definition failed validation", body = crate::error::ApiErrorBody),
    ),
    tag = "template-library",
)]
pub async fn create_template(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Json(body): Json<CreateTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_tenant_admin(&state, ctx.tenant_id, ctx.user_id).await?;

    let template = tenancy::create_template(
        state.identity_pool()?,
        ctx.tenant_id,
        ctx.user_id,
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

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateTemplateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub definition: Option<MetaSchemaDefinition>,
    pub tags: Option<Vec<String>>,
    pub locale: Option<String>,
}

#[utoipa::path(
    put,
    path = "/api/template-library/{id}",
    params(("id" = Uuid, Path, description = "Template ID")),
    request_body = UpdateTemplateRequest,
    responses(
        (status = 200, description = "Template updated", body = TemplateRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Not a tenant owner/admin", body = crate::error::ApiErrorBody),
        (status = 404, description = "Template not found", body = crate::error::ApiErrorBody),
        (status = 422, description = "The template definition failed validation", body = crate::error::ApiErrorBody),
    ),
    tag = "template-library",
)]
pub async fn update_template(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTemplateRequest>,
) -> Result<Json<TemplateRecord>, ApiError> {
    require_tenant_admin(&state, ctx.tenant_id, ctx.user_id).await?;

    let template = tenancy::update_template(
        state.identity_pool()?,
        ctx.tenant_id,
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

#[utoipa::path(
    delete,
    path = "/api/template-library/{id}",
    params(("id" = Uuid, Path, description = "Template ID")),
    responses(
        (status = 204, description = "Template deleted"),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Not a tenant owner/admin", body = crate::error::ApiErrorBody),
        (status = 404, description = "Template not found", body = crate::error::ApiErrorBody),
    ),
    tag = "template-library",
)]
pub async fn delete_template(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    require_tenant_admin(&state, ctx.tenant_id, ctx.user_id).await?;
    tenancy::delete_template(state.identity_pool()?, ctx.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ForkTemplateRequest {
    pub name: String,
}

#[utoipa::path(
    post,
    path = "/api/template-library/{id}/fork",
    operation_id = "fork_template_library_item",
    params(("id" = Uuid, Path, description = "Template ID to fork")),
    request_body = ForkTemplateRequest,
    responses(
        (status = 201, description = "Forked copy of the template, owned by the caller's tenant", body = TemplateRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Not a tenant owner/admin", body = crate::error::ApiErrorBody),
        (status = 404, description = "Source template not found", body = crate::error::ApiErrorBody),
        (status = 409, description = "A template with this name already exists for the tenant", body = crate::error::ApiErrorBody),
    ),
    tag = "template-library",
)]
pub async fn fork_template(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Path(id): Path<Uuid>,
    Json(body): Json<ForkTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_tenant_admin(&state, ctx.tenant_id, ctx.user_id).await?;

    let template = tenancy::fork_template(
        state.identity_pool()?,
        ctx.tenant_id,
        ctx.user_id,
        id,
        body.name,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(template)))
}

#[cfg(test)]
#[path = "../../../tests/http/controllers/template_library.rs"]
mod tests;
