use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::{get, post};
use loco_rs::controller::Routes;
use serde::Serialize;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, SchemaScope};
use crate::metaschema::{MetaSchemaDefinition, VersioningDiff};
use crate::models::content_schemas::{self, SchemaRecord};

#[derive(Debug, Serialize)]
pub struct CreateSchemaResponse {
    pub schema: SchemaRecord,
    pub diff: VersioningDiff,
}

/// Registers a schema definition, or adds it as a new version of an existing one.
///
/// Template references (`{"template_id": "..."}`) are not accepted yet: only an inline
/// `MetaSchemaDefinition`. Templates are a later slice.
pub async fn create_schema(
    authorized: Authorized<SchemaScope>,
    Json(definition): Json<MetaSchemaDefinition>,
) -> Result<(StatusCode, Json<CreateSchemaResponse>), ApiError> {
    let tenant_id = authorized.ctx.tenant_id;
    let workspace_id = authorized.ctx.workspace_id;
    let (schema, diff) =
        content_schemas::create_schema(authorized.txn(), tenant_id, workspace_id, definition)
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
    Path(schema_id): Path<uuid::Uuid>,
) -> Result<Json<SchemaRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record = content_schemas::get_by_id(authorized.txn(), workspace_id, schema_id).await?;
    Ok(Json(record))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/schemas")
        .add("/", post(create_schema))
        .add("/active/{name}", get(get_active_schema))
        .add("/{schema_id}", get(get_schema_by_id))
}
