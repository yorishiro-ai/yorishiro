use axum::Json;
use axum::http::StatusCode;
use axum::routing::post;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, SchemaScope};
use crate::models::import::{self, ImportResult};

/// Line-delimited JSON import in the same format `GET /api/export.jsonl` produces: one
/// `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
/// Requires `SchemaScope` (rather than `WriteScope`) since an import can create schemas, which is
/// itself a schema-scope-only operation elsewhere in the API.
///
/// All-or-nothing: on the first error the request fails with that error and, because the
/// handler never reaches `Authorized::commit()`, nothing imported so far is applied.
pub async fn import_jsonl(
    authorized: Authorized<SchemaScope>,
    body: String,
) -> Result<(StatusCode, Json<ImportResult>), ApiError> {
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
    Ok((StatusCode::OK, Json(result)))
}

pub fn routes() -> Routes {
    Routes::new().add("/api/import.jsonl", post(import_jsonl))
}
