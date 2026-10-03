use axum::Json;
use axum::routing::get;
use loco_rs::controller::Routes;

use crate::controllers::extractors::AuthContext;
use crate::dtos::whoami::WhoAmIResponse;

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/whoami", responses((status = 200, body = super::openapi::WhoAmIResponse), (status = 401, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), tag = "community"))]
pub async fn whoami(AuthContext(ctx): AuthContext) -> Json<WhoAmIResponse> {
    Json(WhoAmIResponse {
        workspace_id: ctx.workspace_id,
        tenant_id: ctx.tenant_id,
        scope: ctx.scope,
        user_id: ctx.user_id,
        audit: ctx.audit,
    })
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![super::route_inventory::path_doc(__path_whoami)]
}

pub fn routes() -> Routes {
    Routes::new().prefix("api/whoami").add("/", get(whoami))
}
