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
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub struct AuditLogRecord {
    pub id: uuid::Uuid,
    pub workspace_id: uuid::Uuid,
    pub tenant_id: uuid::Uuid,

    pub api_key_id: Option<uuid::Uuid>,

    pub user_id: Option<uuid::Uuid>,
    pub action: crate::models::api_key_audit_log::AuditAction,
    pub detail: serde_json::Value,
    #[cfg_attr(feature = "openapi", schemars(with = "chrono::DateTime<chrono::Utc>"))]
    pub created_at: String,
}

#[cfg(feature = "openapi")]
fn list_audit_log_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/audit-log",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-grants" :  serde_json::json!(["audit"]), "x-yorishiro-required-actions" :  serde_json::json!(["audit_log_list"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<AuditLogRecord>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "page", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}, {"name": "page_size", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}]}),
    )
}
pub(crate) async fn list_audit_log(
    authorized: AuditAuthorized,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<AuditLogRecord>>, ApiError> {
    let records = authorized.records(page.try_into()?).await?;
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
            created_at: crate::data::time::in_timezone(&record.created_at, timezone).to_rfc3339(),
        })
        .collect()
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![list_audit_log_doc()]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/audit-log")
        .add("/", get(list_audit_log))
}
