//! The edition seam: whether enterprise behavior is on, and the layer that hides gated routes while it is not.

use std::sync::Arc;

use loco_rs::app::AppContext;

/// Reports whether enterprise-only behavior is currently enabled.
///
/// Base owns this seam, while `src/app.rs` installs the enterprise licence implementation.
pub trait EnterpriseEdition: Send + Sync {
    fn is_active(&self) -> bool;
}

/// Returns whether the running deployment currently has enterprise features enabled.
#[must_use]
pub fn is_active(ctx: &AppContext) -> bool {
    ctx.shared_store
        .get::<Arc<dyn EnterpriseEdition>>()
        .is_some_and(|edition| edition.is_active())
}

/// Refuses a request when no active licence is held, for the routes this is applied to.
///
/// This is the enterprise-edition boundary: one binary carries both editions, and the licence decides at
/// runtime which surfaces answer.
///
/// **Per request, not per boot.** Mounting the gated routes conditionally at startup would be
/// simpler and is wrong: `LicenceState::is_active` compares `exp` against the current time on every
/// call precisely so a key that lapses while the process runs stops unlocking enterprise features without
/// a restart (see `ee::services::licence`). A route set decided once at boot cannot un-mount, which
/// would turn that property into a silent enforcement hole.
///
/// Applied through `Routes::layer`, which wraps each handler's own `MethodRouter`, so it reaches
/// exactly the routes it is attached to and cannot leak onto the community ones. That is a property
/// of the data rather than of this function, but it is the reason those routes stay reachable.
///
/// Running before the handler is also what keeps an unlicensed deployment un-probeable: every gated
/// route answers the same 404 to everyone, rather than authenticating first and thereby confirming
/// to a valid key that the endpoint exists and is merely locked.
#[cfg(feature = "enterprise")]
pub(crate) async fn licence_gate(
    axum::extract::State(ctx): axum::extract::State<AppContext>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    let active = is_active(&ctx);

    if active {
        return next.run(request).await;
    }

    // 404 rather than 402 or 403, matching the setup wizard's answer for a capability this
    // deployment does not offer: the endpoint is genuinely not being served here. The message names
    // the reason, because the operator is the one who can fix it.
    //
    // Rendered through `ApiError` so the body matches every other error this application emits
    // rather than being formatted a second way.
    crate::controllers::error::ApiError(crate::error::YorishiroError::not_found(
        "this feature requires a licence key (set YORISHIRO_LICENSE_KEY)",
    ))
    .into_response()
}
