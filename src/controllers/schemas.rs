//! REST routes for schemas and the built-in templates.

use crate::controllers::Created;
use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;
use serde_json::Value;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{AuthContext, Verified};
use crate::controllers::openapi::Routes;
use crate::data::templates::{self, TemplateSummary};
use crate::dtos::schemas::{CreateSchemaRequest, CreateSchemaResponse};
use crate::models::schema_schemas::metaschema::MetaSchemaDefinition;
use crate::models::schema_schemas::{self, SchemaRecord, SchemaSummary};

pub(crate) async fn archive_schema(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::SchemaIdPath { schema_id }): Path<
        crate::dtos::common::SchemaIdPath<Uuid>,
    >,
) -> Result<Json<SchemaRecord>, ApiError> {
    Ok(Json(
        schema_schemas::archive_for_actor(&ctx, verified.ctx, schema_id).await?,
    ))
}

pub(crate) async fn delete_schema(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::SchemaIdPath { schema_id }): Path<
        crate::dtos::common::SchemaIdPath<Uuid>,
    >,
) -> Result<axum::response::NoContent, ApiError> {
    schema_schemas::delete_for_actor(&ctx, verified.ctx, schema_id).await?;
    Ok(axum::response::NoContent)
}

pub(crate) async fn list_schemas(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<SchemaSummary>>, ApiError> {
    let summaries = schema_schemas::list_for_actor(&ctx, &actor, page.try_into()?).await?;
    Ok(Json(summaries))
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn create_schema(
    State(ctx): State<AppContext>,
    verified: Verified,
    Json(body): Json<CreateSchemaRequest>,
) -> Result<Created<CreateSchemaResponse>, ApiError> {
    Ok(Created(
        schema_schemas::create_for_actor(&ctx, verified.ctx, body).await?,
    ))
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn get_active_schema(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::NamePath { name }): Path<crate::dtos::common::NamePath<String>>,
) -> Result<Json<SchemaRecord>, ApiError> {
    Ok(Json(
        schema_schemas::get_active_for_actor(&ctx, verified.ctx, &name).await?,
    ))
}

pub(crate) async fn get_schema_by_id(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::SchemaIdPath { schema_id }): Path<
        crate::dtos::common::SchemaIdPath<Uuid>,
    >,
) -> Result<Json<SchemaRecord>, ApiError> {
    Ok(Json(
        schema_schemas::get_by_id_for_actor(&ctx, verified.ctx, schema_id).await?,
    ))
}

pub(crate) async fn list_templates(
    _verified: Verified,
) -> Result<Json<Vec<TemplateSummary>>, ApiError> {
    Ok(Json(templates::list_templates()))
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn get_template(
    _verified: Verified,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<String>>,
) -> Result<Json<MetaSchemaDefinition>, ApiError> {
    let definition = templates::get_template(&id)?;
    Ok(Json(definition))
}

pub(crate) async fn get_entity_type_json_schema(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::NameEntityTypePath { name, entity_type }): Path<
        crate::dtos::common::NameEntityTypePath<String, String>,
    >,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        schema_schemas::entity_type_json_schema_for_actor(&ctx, verified.ctx, &name, &entity_type)
            .await?,
    ))
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/schemas")
        .add(
            "/",
            aide::axum::routing::get_with(list_schemas, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_list"]),
                ))
            }),
        )
        .add(
            "/",
            aide::axum::routing::post_with(create_schema, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_create"]),
                ))
            }),
        )
        .add(
            "/active/{name}",
            aide::axum::routing::get_with(get_active_schema, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_get"]),
                ))
            }),
        )
        .add(
            "/{schema_id}",
            aide::axum::routing::get_with(get_schema_by_id, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_get"]),
                ))
            }),
        )
        .add(
            "/{schema_id}",
            aide::axum::routing::delete_with(delete_schema, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_delete"]),
                ))
            }),
        )
        .add(
            "/{schema_id}/archive",
            aide::axum::routing::post_with(archive_schema, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_archive"]),
                ))
            }),
        )
        .add(
            "/active/{name}/entity-types/{entity_type}/json-schema",
            aide::axum::routing::get_with(get_entity_type_json_schema, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_get"]),
                ))
            }),
        )
}

pub(crate) fn template_routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/templates")
        .add("/", aide::axum::routing::get(list_templates))
        .add("/{id}", aide::axum::routing::get(get_template))
}
