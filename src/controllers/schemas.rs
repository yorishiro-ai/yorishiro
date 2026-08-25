use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, SchemaScope};
use crate::error::YorishiroError;
use crate::metaschema::{self, MetaSchemaDefinition, VersioningDiff};
use crate::models::content_schemas::{self, SchemaRecord, SchemaSummary};
use crate::models::identity_templates;
use crate::templates::{self, TemplateSummary};

#[derive(Debug, Serialize)]
pub struct CreateSchemaResponse {
    pub schema: SchemaRecord,
    pub diff: VersioningDiff,
}

pub async fn list_schemas(
    authorized: Authorized<ReadScope>,
    Query(page): Query<crate::controllers::PageParams>,
) -> Result<Json<Vec<SchemaSummary>>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let summaries = content_schemas::list(authorized.txn(), workspace_id, page.into()).await?;
    Ok(Json(summaries))
}

/// Either an inline schema definition, or a reference to a template.
///
/// `template_id` accepts both kinds of template, because a caller holding an id should not have to know which kind it is: a built-in id (`"task-management"`, see `GET /api/templates`) served from the binary, or a UUID from the tenant's template library (`GET /api/template-library`).
///
/// Untagged so existing clients posting a flat `MetaSchemaDefinition` body keep working unchanged.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CreateSchemaRequest {
    Definition(MetaSchemaDefinition),
    Template { template_id: String },
}

pub async fn create_schema(
    State(ctx): State<AppContext>,
    authorized: Authorized<SchemaScope>,
    Json(body): Json<CreateSchemaRequest>,
) -> Result<(StatusCode, Json<CreateSchemaResponse>), ApiError> {
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
                identity_templates::resolve_template_definition(&ctx.db, tenant_id, &template_id)
                    .await?;
            origin_template_id = origin;
            origin_snapshot = origin.map(|_| definition.clone());
            definition
        }
    };

    let workspace_id = authorized.ctx.workspace_id;
    let (schema, diff) = content_schemas::create_schema(
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

pub async fn get_active_schema(
    authorized: Authorized<ReadScope>,
    Path(name): Path<String>,
) -> Result<Json<SchemaRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record = content_schemas::get_active_schema(authorized.txn(), workspace_id, &name).await?;
    Ok(Json(record))
}

pub async fn get_schema_by_id(
    authorized: Authorized<ReadScope>,
    Path(schema_id): Path<Uuid>,
) -> Result<Json<SchemaRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record = content_schemas::get_by_id(authorized.txn(), workspace_id, schema_id).await?;
    Ok(Json(record))
}

pub async fn list_templates(
    _authorized: Authorized<ReadScope>,
) -> Result<Json<Vec<TemplateSummary>>, ApiError> {
    Ok(Json(templates::list_templates()))
}

pub async fn get_template(
    _authorized: Authorized<ReadScope>,
    Path(id): Path<String>,
) -> Result<Json<MetaSchemaDefinition>, ApiError> {
    let definition = templates::get_template(&id)?;
    Ok(Json(definition))
}

pub async fn get_entity_type_json_schema(
    authorized: Authorized<ReadScope>,
    Path((name, entity_type)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record = content_schemas::get_active_schema(authorized.txn(), workspace_id, &name).await?;

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
        .add(
            "/active/{name}/entity-types/{entity_type}/json-schema",
            get(get_entity_type_json_schema),
        )
}

pub fn template_routes() -> Routes {
    Routes::new()
        .prefix("api/templates")
        .add("/", get(list_templates))
        .add("/{id}", get(get_template))
}
