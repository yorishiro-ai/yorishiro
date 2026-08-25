use axum::Json;
use axum::routing::get;
use loco_rs::controller::Routes;
use serde::Serialize;
use uuid::Uuid;

use crate::controllers::extractors::AuthContext;
use crate::services::auth::ApiKeyScope;

#[derive(Serialize)]
pub struct WhoAmIResponse {
    workspace_id: Uuid,
    tenant_id: Uuid,
    scope: ApiKeyScope,
    /// The user this key was issued for, if it was created with `admin create-api-key --user`.
    user_id: Option<Uuid>,
    /// Independent of `scope`: whether this key additionally holds the audit grant, checked separately by `GET /api/audit-log`.
    audit: bool,
}

pub async fn whoami(AuthContext(ctx): AuthContext) -> Json<WhoAmIResponse> {
    Json(WhoAmIResponse {
        workspace_id: ctx.workspace_id,
        tenant_id: ctx.tenant_id,
        scope: ctx.scope,
        user_id: ctx.user_id,
        audit: ctx.audit,
    })
}

pub fn routes() -> Routes {
    Routes::new().prefix("api/whoami").add("/", get(whoami))
}
