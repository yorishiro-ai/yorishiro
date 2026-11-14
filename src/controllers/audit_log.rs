//! `GET /api/audit-log`: the workspace's audit trail, for a key holding the independent `audit` grant.
//!
//! Gated on `audit`, not on `ApiKeyScope`: an ordinary `read`/`write`/`schema`/`migration` key, however high its scope, cannot reach this route without the grant issued separately (see `models::api_keys::AuthContext::audit`'s doc comment for why that grant sits outside the scope ladder).

use axum::Json;
use axum::extract::Query;
use axum::routing::get;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuditAuthorized;

#[derive(serde::Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AuditLogRecord {
    pub id: uuid::Uuid,
    pub workspace_id: uuid::Uuid,
    pub tenant_id: uuid::Uuid,
    #[cfg_attr(feature = "openapi", schema(nullable = true))]
    pub api_key_id: Option<uuid::Uuid>,
    #[cfg_attr(feature = "openapi", schema(nullable = true))]
    pub user_id: Option<uuid::Uuid>,
    pub action: crate::models::api_key_audit_log::AuditAction,
    pub detail: serde_json::Value,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/audit-log", params(("page" = Option<i32>, Query), ("page_size" = Option<i32>, Query)), responses((status = 200, body = [AuditLogRecord]), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-grants" = json!(["audit"])), ("x-yorishiro-required-actions" = json!(["audit_log_list"]))), tag = "community"))]
pub(crate) async fn list_audit_log(
    authorized: AuditAuthorized,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<AuditLogRecord>>, ApiError> {
    let workspace_id = authorized.ctx.require_workspace()?;
    let records = authorized.records(workspace_id, page.try_into()?).await?;
    Ok(Json(render_records(records, authorized.timezone())))
}

pub(crate) fn render_records(
    records: Vec<crate::models::_entities::api_key_audit_log::Model>,
    timezone: chrono_tz::Tz,
) -> Vec<AuditLogRecord> {
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
            created_at: crate::time::in_timezone(record.created_at, timezone).to_rfc3339(),
        })
        .collect()
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
