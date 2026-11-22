//! Shared request budgets and per-router credential rate limiting.

use std::sync::Arc;

use loco_rs::{Result, app::AppContext};

use crate::controllers::middleware::rate_limit::{self, RateLimiter};
use crate::data::settings::Settings;

pub(crate) fn install(ctx: &AppContext, settings: &Settings) {
    ctx.shared_store
        .insert(Arc::new(RateLimiter::search(settings)));
}

pub(crate) fn after_routes(
    router: axum::Router,
    ctx: &AppContext,
    credential_paths: &[&'static str],
) -> Result<axum::Router> {
    let settings = crate::data::settings::configured(ctx)?;
    let limiter = Arc::new(RateLimiter::auth(&settings).guarding(credential_paths));
    Ok(router.layer(axum::middleware::from_fn_with_state(
        limiter,
        rate_limit::enforce,
    )))
}
