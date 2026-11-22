//! REST routes for the workspace template library.

use crate::controllers::Created;
use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
use crate::dtos::template_library::{
    CreateTemplateRequest, ForkTemplateRequest, UpdateTemplateRequest,
};
use crate::models::template_templates::{self, TemplateRecord};

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

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn get_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<Json<TemplateRecord>, ApiError> {
    let template = template_templates::get_as(&ctx.db, &auth, id).await?;
    Ok(Json(template))
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn create_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<CreateTemplateRequest>,
) -> Result<impl aide::axum::IntoApiResponse, ApiError> {
    let template = template_templates::create_as(&ctx.db, &auth, body.into()).await?;
    Ok(Created(template))
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn update_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Json(body): Json<UpdateTemplateRequest>,
) -> Result<Json<TemplateRecord>, ApiError> {
    let template = template_templates::update_as(&ctx.db, &auth, id, body.into()).await?;
    Ok(Json(template))
}

pub(crate) async fn delete_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    template_templates::delete_as(&ctx.db, &auth, id).await?;
    Ok(axum::response::NoContent)
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn fork_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Json(body): Json<ForkTemplateRequest>,
) -> Result<impl aide::axum::IntoApiResponse, ApiError> {
    let template = template_templates::fork_as(&ctx.db, &auth, id, body.name).await?;
    Ok(Created(template))
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/template-library")
        .add(
            "/",
            aide::axum::routing::get_with(list_templates, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["template_library_list"]),
                ))
            }),
        )
        .add(
            "/",
            aide::axum::routing::post_with(create_template, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["template_library_create"]),
                ))
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::get_with(get_template, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["template_library_get"]),
                ))
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::put_with(update_template, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["template_library_update"]),
                ))
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::delete_with(delete_template, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["template_library_delete"]),
                ))
            }),
        )
        .add(
            "/{id}/fork",
            aide::axum::routing::post_with(fork_template, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["template_library_fork"]),
                ))
            }),
        )
}
