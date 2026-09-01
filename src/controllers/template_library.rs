use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::Deserialize;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::members::require_tenant_admin;
use crate::metaschema::MetaSchemaDefinition;
use crate::models::identity_templates::{
    self, CreateTemplateInput, TemplateRecord, UpdateTemplateInput,
};

pub async fn list_templates(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Query(page): Query<crate::controllers::PageParams>,
) -> Result<Json<Vec<TemplateRecord>>, ApiError> {
    let templates =
        identity_templates::list_templates(&ctx.db, auth.tenant_id, page.into()).await?;
    Ok(Json(templates))
}

pub async fn get_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<TemplateRecord>, ApiError> {
    let template = identity_templates::get_template(&ctx.db, auth.tenant_id, id).await?;
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

pub async fn create_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<CreateTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;

    let template = identity_templates::create_template(
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

pub async fn update_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTemplateRequest>,
) -> Result<Json<TemplateRecord>, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;

    let template = identity_templates::update_template(
        &ctx.db,
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
    Ok(Json(template))
}

pub async fn delete_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;
    identity_templates::delete_template(&ctx.db, auth.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct ForkTemplateRequest {
    pub name: String,
}

pub async fn fork_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
    Json(body): Json<ForkTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;

    let template =
        identity_templates::fork_template(&ctx.db, auth.tenant_id, auth.user_id, id, body.name)
            .await?;
    Ok((StatusCode::CREATED, Json(template)))
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
