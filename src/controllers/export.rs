use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope};
use crate::error::ResultExt;
use crate::models::export;

/// Line-delimited JSON export of every schema, entity, and relation belonging to the workspace, one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
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

pub fn routes() -> Routes {
    Routes::new().add("/api/export.jsonl", get(export_jsonl))
}
