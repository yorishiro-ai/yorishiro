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
#[cfg(feature = "openapi")]
fn whoami_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/whoami",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<WhoAmIResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn whoami(
    State(app): State<AppContext>,
    AuthContext(ctx): AuthContext,
) -> Result<Json<WhoAmIResponse>, super::ApiError> {
    Ok(Json(
        crate::models::iam::access::identity(&app.db, &ctx)
            .await?
            .into(),
    ))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![whoami_doc()]
}

pub(crate) fn routes() -> Routes {
    Routes::new().prefix("api/whoami").add("/", get(whoami))
}
