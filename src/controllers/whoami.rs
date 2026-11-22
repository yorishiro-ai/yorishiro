use axum::Json;
use axum::extract::State;
use loco_rs::app::AppContext;

use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
use crate::dtos::whoami::WhoAmIResponse;

/// Returns the current credential's identity and selected target.
///
/// # Errors
/// Rejects stale credentials or invalid target bindings and propagates lookup failures.
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

pub(crate) fn routes() -> Routes {
    Routes::new().prefix("api/whoami").add(
        "/",
        aide::axum::routing::get_with(whoami, |op| {
            op.tag("community")
                .security_requirement("bearer_auth")
                .response::<401, Json<super::openapi::ApiErrorBody>>()
        }),
    )
}
