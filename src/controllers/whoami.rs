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

#[utoipa::path(get, path = "/api/whoami", responses((status = 200, body = super::openapi::WhoAmIResponse), (status = 401, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), tag = "community")]
pub async fn whoami(AuthContext(ctx): AuthContext) -> Json<WhoAmIResponse> {
    Json(WhoAmIResponse {
        workspace_id: ctx.workspace_id,
        tenant_id: ctx.tenant_id,
        scope: ctx.scope,
        user_id: ctx.user_id,
        audit: ctx.audit,
    })
}

pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![super::route_inventory::path_doc(__path_whoami)]
}

pub fn routes() -> Routes {
    Routes::new().prefix("api/whoami").add("/", get(whoami))
}
