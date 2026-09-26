use axum::Json;
use axum::routing::post;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, SchemaScope};
use crate::models::import::{self, ImportResult};

/// Line-delimited JSON import in the same format `GET /api/export.jsonl` produces: one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
/// Requires `SchemaScope` (rather than `WriteScope`) since an import can create schemas, which is itself a schema-scope-only operation elsewhere in the API.
///
/// All-or-nothing: on the first error the request fails with that error and, because the handler never reaches `Authorized::commit()`, nothing imported so far is applied.
#[utoipa::path(post, path = "/api/import.jsonl", request_body(content = String, content_type = "application/x-ndjson"), responses((status = 200, body = super::openapi::ImportResult), (status = 401, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "community")]
pub async fn import_jsonl(
    authorized: Authorized<SchemaScope>,
    body: String,
) -> Result<Json<ImportResult>, ApiError> {
    let tenant_id = authorized.ctx.tenant_id;
    let workspace_id = authorized.ctx.workspace_id;
    let imported_by = authorized.ctx.user_id;
    let result = import::import_jsonl(
        authorized.txn(),
        tenant_id,
        workspace_id,
        imported_by,
        body.as_bytes(),
    )
    .await?;
    authorized.commit().await?;
    Ok(Json(result))
}

pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![super::route_inventory::path_doc(__path_import_jsonl)]
}

pub fn routes() -> Routes {
    Routes::new().add("/api/import.jsonl", post(import_jsonl))
}
