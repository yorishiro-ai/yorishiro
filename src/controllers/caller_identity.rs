use axum::Json;
use axum::extract::State;
use loco_rs::app::AppContext;

use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
use crate::dtos::caller_identity::CallerIdentityResponse;

/// Returns the current credential's identity and selected target.
///
/// # Errors
/// Rejects stale credentials and propagates lookup failures.
async fn get_caller_identity(
    State(app): State<AppContext>,
    AuthContext(ctx): AuthContext,
) -> Result<Json<CallerIdentityResponse>, super::ApiError> {
    Ok(Json(
        crate::models::iam::access::identity(&app.db, &ctx)
            .await?
            .into(),
    ))
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/caller-identity")
        .add(
            "/",
            aide::axum::routing::get_with(get_caller_identity, |op| op.id("GetCallerIdentity")),
        )
}
