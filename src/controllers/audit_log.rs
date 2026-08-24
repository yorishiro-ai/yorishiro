//! `GET /api/audit-log`: the workspace's audit trail, for a key holding the independent `audit` grant.
//!
//! Gated on `audit`, not on `ApiKeyScope`: an ordinary `read`/`write`/`schema`/`migration` key, however high its scope, cannot reach this route without the grant issued separately (see `services::auth::AuthContext::audit`'s doc comment for why that grant sits outside the scope ladder).

use axum::Json;
use axum::extract::Query;
use axum::routing::get;
use loco_rs::controller::Routes;
use serde::Deserialize;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuditAuthorized;
use crate::models::identity_api_key_audit_log::{self, Model as AuditLogRecord};

#[derive(Debug, Deserialize)]
pub struct ListAuditLogParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn list_audit_log(
    authorized: AuditAuthorized,
    Query(params): Query<ListAuditLogParams>,
) -> Result<Json<Vec<AuditLogRecord>>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let records = identity_api_key_audit_log::list_for_workspace(
        authorized.txn(),
        workspace_id,
        params.limit.unwrap_or(50),
        params.offset.unwrap_or(0),
    )
    .await?;
    Ok(Json(records))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/audit-log")
        .add("/", get(list_audit_log))
}
