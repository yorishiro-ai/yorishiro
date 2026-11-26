//! `GET /api/audit-log`: the workspaces' audit trail, subject to IAM authorization.

use axum::Json;
use axum::extract::{Query, State};
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::extractors::Verified;
use crate::controllers::openapi::Routes;

#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct AuditLogRecord {
    pub id: uuid::Uuid,
    pub workspace_id: uuid::Uuid,
    pub tenant_id: uuid::Uuid,

    pub access_key_id: Option<uuid::Uuid>,

    pub user_id: Option<uuid::Uuid>,
    pub action: crate::models::access_key_audit_log::AuditAction,
    pub detail: serde_json::Value,
    #[schemars(with = "chrono::DateTime<chrono::Utc>")]
    pub created_at: String,
}

pub(crate) async fn list_audit_log(
    State(ctx): State<AppContext>,
    verified: Verified,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<AuditLogRecord>>, ApiError> {
    let records =
        crate::models::access_key_audit_log::list_for_actor(&ctx, &verified.ctx, page.try_into()?)
            .await?;
    Ok(Json(render_records(
        records,
        crate::data::time::configured(&ctx),
    )?))
}

pub(crate) fn render_records(
    records: Vec<crate::models::_entities::access_key_audit_log::Model>,
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
                access_key_id: record.access_key_id,
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
        aide::axum::routing::get_with(list_audit_log, |op| op.id("ListWorkspaceAuditEvents")),
    )
}
