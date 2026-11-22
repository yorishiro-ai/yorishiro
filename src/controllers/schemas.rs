//! REST routes for schemas and the built-in templates.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde_json::Value;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{AuthContext, Authorized, ReadScope, SchemaScope, Verified};
use crate::data::templates::{self, TemplateSummary};
use crate::dtos::schemas::{CreateSchemaRequest, CreateSchemaResponse};
use crate::error::YorishiroError;
use crate::models::api_keys::ApiKeyScope;
use crate::models::schema_schemas::metaschema::{self, MetaSchemaDefinition};
use crate::models::schema_schemas::{self, SchemaRecord, SchemaSummary};
use crate::models::template_templates;

#[cfg(feature = "openapi")]
fn archive_schema_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schemas/{schema_id}/archive",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_archive"] ), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<SchemaRecord>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "schema_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
pub(crate) async fn archive_schema(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(schema_id): Path<Uuid>,
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

#[cfg(feature = "openapi")]
fn delete_schema_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schemas/{schema_id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_delete"] ), "responses": {"204": serde_json::json!({"description": "Schema version deleted"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "409": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "schema_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
pub(crate) async fn delete_schema(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(schema_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
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
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
fn list_schemas_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schemas",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_list"] ), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<SchemaSummary>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "X-Workspace-Id", "in": "header", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}, {"name": "page", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}, {"name": "page_size", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}]}),
    )
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

#[cfg(feature = "openapi")]
fn create_schema_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schemas",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::CreateSchemaRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_create"] ), "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<CreateSchemaResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "409": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "X-Workspace-Id", "in": "header", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn create_schema(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Json(body): Json<CreateSchemaRequest>,
) -> Result<(StatusCode, Json<CreateSchemaResponse>), ApiError> {
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
    Ok((
        StatusCode::CREATED,
        Json(CreateSchemaResponse { schema, diff }),
    ))
}

#[cfg(feature = "openapi")]
fn get_active_schema_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schemas/active/{name}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_get"] ), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<SchemaRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "name", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}]}),
    )
}
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn get_active_schema(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(name): Path<String>,
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

#[cfg(feature = "openapi")]
fn get_schema_by_id_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schemas/{schema_id}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_get"] ), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<SchemaRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "schema_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
pub(crate) async fn get_schema_by_id(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(schema_id): Path<Uuid>,
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

#[cfg(feature = "openapi")]
fn list_templates_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/templates",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<TemplateSummary>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
pub(crate) async fn list_templates(
    _verified: Verified<ReadScope>,
) -> Result<Json<Vec<TemplateSummary>>, ApiError> {
    Ok(Json(templates::list_templates()))
}

#[cfg(feature = "openapi")]
fn get_template_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/templates/{id}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::JsonSchema>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}]}),
    )
}
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn get_template(
    _verified: Verified<ReadScope>,
    Path(id): Path<String>,
) -> Result<Json<MetaSchemaDefinition>, ApiError> {
    let definition = templates::get_template(&id)?;
    Ok(Json(definition))
}

#[cfg(feature = "openapi")]
fn get_entity_type_json_schema_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schemas/active/{name}/entity-types/{entity_type}/json-schema",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["schema_get"] ), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::JsonSchema>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "name", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}, {"name": "entity_type", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}]}),
    )
}
pub(crate) async fn get_entity_type_json_schema(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path((name, entity_type)): Path<(String, String)>,
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
        .prefix("api/schemas")
        .add("/", get(list_schemas))
        .add("/", post(create_schema))
        .add("/active/{name}", get(get_active_schema))
        .add("/{schema_id}", get(get_schema_by_id))
        .add("/{schema_id}", delete(delete_schema))
        .add("/{schema_id}/archive", post(archive_schema))
        .add(
            "/active/{name}/entity-types/{entity_type}/json-schema",
            get(get_entity_type_json_schema),
        )
}

pub(crate) fn template_routes() -> Routes {
    Routes::new()
        .prefix("api/templates")
        .add("/", get(list_templates))
        .add("/{id}", get(get_template))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![
        list_schemas_doc(),
        create_schema_doc(),
        archive_schema_doc(),
        delete_schema_doc(),
        get_active_schema_doc(),
        get_schema_by_id_doc(),
        get_entity_type_json_schema_doc(),
    ]
}

#[cfg(feature = "openapi")]
pub(crate) fn template_openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![list_templates_doc(), get_template_doc()]
}
