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
        crate::dtos::common::SchemaIdPath<String>,
    >,
) -> Result<Json<SchemaRecord>, ApiError> {
    let schema_id = super::action_resource_id(&schema_id, ":archive")?;
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

async fn create_schema(
    State(ctx): State<AppContext>,
    verified: Verified,
    Json(body): Json<CreateSchemaRequest>,
) -> Result<Created<CreateSchemaResponse>, ApiError> {
    Ok(Created(
        schema_schemas::create_for_actor(&ctx, verified.ctx, body).await?,
    ))
}

async fn get_active_schema(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::SchemaNamePath { schema_name }): Path<
        crate::dtos::common::SchemaNamePath,
    >,
) -> Result<Json<SchemaRecord>, ApiError> {
    Ok(Json(
        schema_schemas::get_active_for_actor(&ctx, verified.ctx, &schema_name).await?,
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
    State(ctx): State<AppContext>,
    verified: Verified,
) -> Result<Json<Vec<TemplateSummary>>, ApiError> {
    crate::models::template_templates::authorize_builtin(
        &ctx.db,
        &verified.ctx,
        crate::models::iam::policy::Action::ListBuiltinTemplates,
    )
    .await?;
    Ok(Json(templates::list_templates()))
}

async fn get_template(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::TemplateIdPath { template_id }): Path<
        crate::dtos::common::TemplateIdPath<String>,
    >,
) -> Result<Json<MetaSchemaDefinition>, ApiError> {
    crate::models::template_templates::authorize_builtin(
        &ctx.db,
        &verified.ctx,
        crate::models::iam::policy::Action::GetBuiltinTemplate,
    )
    .await?;
    let definition = templates::get_template(&template_id)?;
    Ok(Json(definition))
}

pub(crate) async fn get_entity_type_json_schema(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::SchemaNameEntityTypePath {
        schema_name,
        entity_type,
    }): Path<crate::dtos::common::SchemaNameEntityTypePath>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        schema_schemas::entity_type_json_schema_for_actor(
            &ctx,
            verified.ctx,
            &schema_name,
            &entity_type,
        )
        .await?,
    ))
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/schemas")
        .add(
            "/",
            aide::axum::routing::get_with(list_schemas, |op| op.id("ListSchemas")),
        )
        .add(
            "/",
            aide::axum::routing::post_with(create_schema, |op| op.id("CreateSchema")),
        )
        .add(
            "/active/{schema_name}",
            aide::axum::routing::get_with(get_active_schema, |op| op.id("GetActiveSchema")),
        )
        .add(
            "/{schema_id}",
            aide::axum::routing::get_with(get_schema_by_id, |op| op.id("GetSchema")),
        )
        .add(
            "/{schema_id}",
            aide::axum::routing::delete_with(delete_schema, |op| op.id("DeleteSchema")),
        )
        .add_action(
            "/{schema_id}",
            ":archive",
            aide::axum::routing::post_with(archive_schema, |op| op.id("ArchiveSchema")),
        )
        .add(
            "/active/{schema_name}/entity-types/{entity_type}/json-schema",
            aide::axum::routing::get_with(get_entity_type_json_schema, |op| {
                op.id("GetEntityTypeJsonSchema")
            }),
        )
}

pub(crate) fn template_routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/templates")
        .add(
            "/",
            aide::axum::routing::get_with(list_templates, |op| op.id("ListBuiltinTemplates")),
        )
        .add(
            "/{template_id}",
            aide::axum::routing::get_with(get_template, |op| op.id("GetBuiltinTemplate")),
        )
}
