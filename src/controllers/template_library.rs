//! REST routes for the workspace template library.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
use crate::dtos::template_library::{
    CreateTemplateRequest, ForkTemplateRequest, UpdateTemplateRequest,
};
use crate::models::template_templates::{
    self, CreateTemplateInput, TemplateRecord, UpdateTemplateInput,
};

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

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn update_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
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

pub(crate) async fn delete_template(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<StatusCode, ApiError> {
    template_templates::delete_as(&ctx.db, &auth, id).await?;
    Ok(StatusCode::NO_CONTENT)
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
    Ok((StatusCode::CREATED, Json(template)))
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
                .response::<401, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/",
            aide::axum::routing::post_with(create_template, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["template_library_create"]),
                ))
                .response::<201, Json<TemplateRecord>>()
                .response::<401, Json<super::openapi::ApiErrorBody>>()
                .response::<403, Json<super::openapi::ApiErrorBody>>()
                .response::<422, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::get_with(get_template, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["template_library_get"]),
                ))
                .response::<401, Json<super::openapi::ApiErrorBody>>()
                .response::<404, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::put_with(update_template, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["template_library_update"]),
                ))
                .response::<401, Json<super::openapi::ApiErrorBody>>()
                .response::<403, Json<super::openapi::ApiErrorBody>>()
                .response::<404, Json<super::openapi::ApiErrorBody>>()
                .response::<422, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::delete_with(delete_template, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["template_library_delete"]),
                ))
                .response_with::<204, (), _>(|op| op.description("Template deleted"))
                .response::<401, Json<super::openapi::ApiErrorBody>>()
                .response::<403, Json<super::openapi::ApiErrorBody>>()
                .response::<404, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}/fork",
            aide::axum::routing::post_with(fork_template, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["template_library_fork"]),
                ))
                .response::<201, Json<TemplateRecord>>()
                .response::<401, Json<super::openapi::ApiErrorBody>>()
                .response::<403, Json<super::openapi::ApiErrorBody>>()
                .response::<404, Json<super::openapi::ApiErrorBody>>()
                .response::<422, Json<super::openapi::ApiErrorBody>>()
            }),
        )
}
