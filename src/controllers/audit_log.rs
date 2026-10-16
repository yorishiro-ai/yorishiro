//! `GET /api/audit-log`: the workspace's audit trail, for a key holding the independent `audit` grant.
//!
//! Gated on `audit`, not on `ApiKeyScope`: an ordinary `read`/`write`/`schema`/`migration` key, however high its scope, cannot reach this route without the grant issued separately (see `models::api_keys::AuthContext::audit`'s doc comment for why that grant sits outside the scope ladder).

use axum::Json;
use axum::extract::Query;
use axum::routing::get;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuditAuthorized;
use crate::controllers::openapi::AuditLogRecord;

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/audit-log", params(("page" = Option<i32>, Query), ("page_size" = Option<i32>, Query)), responses((status = 200, body = [super::openapi::AuditLogRecord]), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-grants" = json!(["audit"]))), tag = "community"))]
pub(crate) async fn list_audit_log(
    authorized: AuditAuthorized,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<AuditLogRecord>>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let records = authorized.records(workspace_id, page.into()).await?;
    Ok(Json(
        records
            .into_iter()
            .map(|record| AuditLogRecord {
                id: record.id,
                workspace_id: record.workspace_id,
                tenant_id: record.tenant_id,
                api_key_id: record.api_key_id,
                user_id: record.user_id,
                action: record
                    .action
                    .parse()
                    .expect("audit action matches the DB constraint"),
                detail: record.detail,
                created_at: crate::time::in_timezone(record.created_at, authorized.timezone())
                    .to_rfc3339(),
            })
            .collect(),
    ))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![super::route_inventory::path_doc(__path_list_audit_log)]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/audit-log")
        .add("/", get(list_audit_log))
}
