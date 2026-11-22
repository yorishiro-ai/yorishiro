//! REST routes for schemas and the built-in templates.

use crate::controllers::Created;
use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;
use serde_json::Value;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{AuthContext, Authorized, ReadScope, SchemaScope, Verified};
use crate::controllers::openapi::Routes;
use crate::data::templates::{self, TemplateSummary};
use crate::dtos::schemas::{CreateSchemaRequest, CreateSchemaResponse};
use crate::error::YorishiroError;
use crate::models::api_keys::ApiKeyScope;
use crate::models::schema_schemas::metaschema::{self, MetaSchemaDefinition};
use crate::models::schema_schemas::{self, SchemaRecord, SchemaSummary};
use crate::models::template_templates;

pub(crate) async fn archive_schema(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(crate::dtos::common::SchemaIdPath { schema_id }): Path<
        crate::dtos::common::SchemaIdPath<Uuid>,
    >,
) -> Result<Json<SchemaRecord>, ApiError> {
    let mut authorized = Authorized::<SchemaScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Schema(schema_id),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaArchive)
        .await?;
    let schema = schema_schemas::archive(
        authorized.txn(),
        authorized.ctx.require_workspace()?,
        schema_id,
    )
    .await?;
    authorized.commit().await?;
    Ok(Json(schema))
}

pub(crate) async fn delete_schema(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(crate::dtos::common::SchemaIdPath { schema_id }): Path<
        crate::dtos::common::SchemaIdPath<Uuid>,
    >,
) -> Result<axum::response::NoContent, ApiError> {
    let mut authorized = Authorized::<SchemaScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Schema(schema_id),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaDelete)
        .await?;
    schema_schemas::delete(
        authorized.txn(),
        authorized.ctx.require_workspace()?,
        schema_id,
    )
    .await?;
    authorized.commit().await?;
    Ok(axum::response::NoContent)
}

pub(crate) async fn list_schemas(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<SchemaSummary>>, ApiError> {
    crate::controllers::middleware::auth::require_scope(&actor, ApiKeyScope::Read)?;
    let summaries = schema_schemas::list_for_actor(&ctx, &actor, page.try_into()?).await?;
    Ok(Json(summaries))
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn create_schema(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Json(body): Json<CreateSchemaRequest>,
) -> Result<Created<CreateSchemaResponse>, ApiError> {
    let mut authorized = Authorized::<SchemaScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::SchemaCreate,
        ),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaCreate)
        .await?;
    let tenant_id = authorized.ctx.tenant_id;

    // Carried alongside the definition so the schema can record which library template it came from.
    // A built-in has no row to point at, so it stays None.
    let mut origin_template_id = None;
    // What the template said, kept as the merge base.
    // Only a template body has one: a definition posted inline is not a copy of anything, even when a template of the same name exists.
    let mut origin_snapshot = None;
    let definition = match body {
        CreateSchemaRequest::Definition(definition) => definition,
        CreateSchemaRequest::Template { template_id } => {
            let (definition, origin) =
                template_templates::resolve_template_definition(&ctx.db, tenant_id, &template_id)
                    .await?;
            origin_template_id = origin;
            origin_snapshot = origin.map(|_| definition.clone());
            definition
        }
    };

    let workspace_id = authorized.ctx.require_workspace()?;
    let (schema, diff) = schema_schemas::create_schema(
        authorized.txn(),
        tenant_id,
        workspace_id,
        definition,
        origin_template_id,
        origin_snapshot,
    )
    .await?;
    authorized.commit().await?;
    Ok(Created(CreateSchemaResponse { schema, diff }))
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn get_active_schema(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(crate::dtos::common::NamePath { name }): Path<crate::dtos::common::NamePath<String>>,
) -> Result<Json<SchemaRecord>, ApiError> {
    let mut authorized = Authorized::<ReadScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::SchemaGet,
        ),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaGet)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    let record = schema_schemas::get_active_schema(authorized.txn(), workspace_id, &name).await?;
    Ok(Json(record))
}

pub(crate) async fn get_schema_by_id(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(crate::dtos::common::SchemaIdPath { schema_id }): Path<
        crate::dtos::common::SchemaIdPath<Uuid>,
    >,
) -> Result<Json<SchemaRecord>, ApiError> {
    let mut authorized = Authorized::<ReadScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Schema(schema_id),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaGet)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    let record = schema_schemas::get_by_id(authorized.txn(), workspace_id, schema_id).await?;
    Ok(Json(record))
}

pub(crate) async fn list_templates(
    _verified: Verified<ReadScope>,
) -> Result<Json<Vec<TemplateSummary>>, ApiError> {
    Ok(Json(templates::list_templates()))
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn get_template(
    _verified: Verified<ReadScope>,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<String>>,
) -> Result<Json<MetaSchemaDefinition>, ApiError> {
    let definition = templates::get_template(&id)?;
    Ok(Json(definition))
}

pub(crate) async fn get_entity_type_json_schema(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(crate::dtos::common::NameEntityTypePath { name, entity_type }): Path<
        crate::dtos::common::NameEntityTypePath<String, String>,
    >,
) -> Result<Json<Value>, ApiError> {
    let mut authorized = Authorized::<ReadScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::SchemaGet,
        ),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaGet)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    let record = schema_schemas::get_active_schema(authorized.txn(), workspace_id, &name).await?;

    let entity_type_def = record
        .definition
        .entity_types
        .get(&entity_type)
        .ok_or_else(|| {
            YorishiroError::not_found(format!(
                "entity_type '{entity_type}' not found in schema '{name}'"
            ))
        })?;

    Ok(Json(metaschema::entity_type_to_json_schema(
        entity_type_def,
    )))
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
