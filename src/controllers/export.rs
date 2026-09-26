use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope};
use crate::error::ResultExt;
use crate::models::export;

/// Line-delimited JSON export of every schema, entity, and relation belonging to the workspace, one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
#[utoipa::path(get, path = "/api/export.jsonl", responses((status = 200, description = "Newline-delimited JSON export", content_type = "application/x-ndjson", body = String), (status = 401, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"]))), tag = "community")]
pub async fn export_jsonl(
    authorized: Authorized<ReadScope>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let records = export::export_all(authorized.txn(), workspace_id).await?;

    let mut body = Vec::new();
    for record in &records {
        serde_json::to_writer(&mut body, record).internal()?;
        body.push(b'\n');
    }

    Ok(([(header::CONTENT_TYPE, "application/x-ndjson")], body))
}

pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![super::route_inventory::path_doc(__path_export_jsonl)]
}

pub fn routes() -> Routes {
    Routes::new().add("/api/export.jsonl", get(export_jsonl))
}
