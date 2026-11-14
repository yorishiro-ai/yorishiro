use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope};
use crate::models::export;

/// Line-delimited JSON export of every schema, entity, and relation belonging to the workspace, one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/export.jsonl", responses((status = 200, description = "Newline-delimited JSON export, at most 8 MiB and 10,000 records", content_type = "application/x-ndjson", body = String), (status = 401, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["export"]))), tag = "community"))]
pub(crate) async fn export_jsonl(
    mut authorized: Authorized<ReadScope>,
) -> Result<impl IntoResponse, ApiError> {
    authorized
        .require_action(crate::models::iam_policy::Action::Export)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    let body = export::export_jsonl(authorized.txn(), workspace_id).await?;

    Ok(([(header::CONTENT_TYPE, "application/x-ndjson")], body))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![super::route_inventory::path_doc(__path_export_jsonl)]
}

pub fn routes() -> Routes {
    Routes::new().add("/api/export.jsonl", get(export_jsonl))
}
