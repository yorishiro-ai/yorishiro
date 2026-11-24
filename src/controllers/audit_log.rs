//! `GET /api/audit-log`: the workspace's audit trail, for a key holding the independent `audit` grant.
//!
//! Gated on `audit`, not on `ApiKeyScope`: an ordinary `read`/`write`/`schema`/`migration` key, however high its scope, cannot reach this route without the grant issued separately (see `models::api_keys::AuthContext::audit`'s doc comment for why that grant sits outside the scope ladder).

use axum::Json;
use axum::extract::Query;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuditAuthorized;
use crate::controllers::openapi::Routes;

#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct AuditLogRecord {
    pub id: uuid::Uuid,
    pub workspace_id: uuid::Uuid,
    pub tenant_id: uuid::Uuid,

    pub api_key_id: Option<uuid::Uuid>,

    pub user_id: Option<uuid::Uuid>,
    pub action: crate::models::api_key_audit_log::AuditAction,
    pub detail: serde_json::Value,
    #[schemars(with = "chrono::DateTime<chrono::Utc>")]
    pub created_at: String,
}

pub(crate) async fn list_audit_log(
    authorized: AuditAuthorized,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<AuditLogRecord>>, ApiError> {
    let records = authorized.records(page.try_into()?).await?;
    Ok(Json(render_records(records, authorized.timezone())?))
}

pub(crate) fn render_records(
    records: Vec<crate::models::_entities::api_key_audit_log::Model>,
    timezone: chrono_tz::Tz,
) -> Result<Vec<AuditLogRecord>, crate::YorishiroError> {
    records
        .into_iter()
        .map(|record| {
            let action = record.action.parse().map_err(|_| {
                crate::YorishiroError::Internal(
                    format!("unknown audit action '{}' in audit log", record.action).into(),
                )
            })?;
            Ok(AuditLogRecord {
                id: record.id,
                workspace_id: record.workspace_id,
                tenant_id: record.tenant_id,
                api_key_id: record.api_key_id,
                user_id: record.user_id,
                action,
                detail: record.detail,
                created_at: crate::data::time::in_timezone(&record.created_at, timezone)
                    .to_rfc3339(),
            })
        })
        .collect()
}

pub fn routes() -> Routes {
    Routes::new().tag("community").prefix("api/audit-log").add(
        "/",
        aide::axum::routing::get_with(list_audit_log, |op| {
            op.with(crate::controllers::openapi::extension(
                "x-yorishiro-required-grants",
                serde_json::json!(["audit"]),
            ))
            .with(crate::controllers::openapi::extension(
                "x-yorishiro-required-actions",
                serde_json::json!(["audit_log_list"]),
            ))
        }),
    )
}
