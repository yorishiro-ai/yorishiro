use axum::Json;
use axum::http::StatusCode;
use yorishiro_core::repositories::import::{self, ImportResult};

use crate::error::ApiError;
use crate::http::middleware::auth::{Authorized, SchemaScope};

/// Line-delimited JSON import in the same format `GET /api/export.jsonl` produces: one
/// `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line. Requires
/// `SchemaScope` (rather than `WriteScope`) since an import can create schemas, which is
/// itself a schema-scope-only operation elsewhere in the API.
///
/// Runs as a single transaction: either every record in the body is applied, or (on the
/// first error) none of it is and the request fails with that error.
#[utoipa::path(
    post,
    path = "/api/import.jsonl",
    request_body(content = String, content_type = "application/x-ndjson", description = "JSON Lines document in the export format"),
    responses(
        (status = 200, description = "Every record in the body was imported successfully", body = ImportResult),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 422, description = "A line was malformed, or a record failed validation (e.g. unknown schema/entity reference)", body = crate::error::ApiErrorBody),
    ),
    tag = "export",
)]
pub async fn import_jsonl(
    mut authorized: Authorized<SchemaScope>,
    body: String,
) -> Result<(StatusCode, Json<ImportResult>), ApiError> {
    let tenant_id = authorized.ctx.tenant_id;
    let workspace_id = authorized.ctx.workspace_id;
    let result =
        import::import_jsonl(authorized.conn(), tenant_id, workspace_id, body.as_bytes()).await?;
    Ok((StatusCode::OK, Json(result)))
}

#[cfg(test)]
#[path = "../../../tests/http/controllers/import.rs"]
mod tests;
