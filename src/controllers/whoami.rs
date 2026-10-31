use axum::Json;
use axum::extract::State;
use axum::routing::get;
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::extractors::AuthContext;
use crate::dtos::whoami::WhoAmIResponse;

/// Returns the current credential's identity and selected target.
///
/// # Errors
/// Rejects stale credentials or invalid target bindings and propagates lookup failures.
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/whoami", responses((status = 200, body = crate::dtos::whoami::WhoAmIResponse), (status = 401, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), tag = "community"))]
pub async fn whoami(
    State(app): State<AppContext>,
    AuthContext(ctx): AuthContext,
) -> Result<Json<WhoAmIResponse>, super::ApiError> {
    Ok(Json(
        crate::models::access::identity(&app.db, &ctx).await?.into(),
    ))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![super::route_inventory::path_doc(__path_whoami)]
}

pub fn routes() -> Routes {
    Routes::new().prefix("api/whoami").add("/", get(whoami))
}
