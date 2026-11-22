//! Licence state, composed after the base installer.
//!
//! Authentication is edition-neutral: key resolution, including tenant-scoped keys, is the base `DefaultAuthenticator`, so the overlay no longer installs its own authenticator.

use std::sync::Arc;

use loco_rs::app::AppContext;

use crate::db::is_sqlite;
use crate::edition::ee::controllers::middleware::edition::LicenceState;

pub(crate) fn install(ctx: &AppContext) {
    ctx.shared_store
        .insert(Arc::new(LicenceState::from_config(&ctx.config)));
    if is_sqlite(&ctx.db) {
        tracing::warn!(
            "browsing the marketplace and publishing a template version are unavailable on SQLite \
             and return backend_unsupported (501). Point DATABASE_URL at PostgreSQL to use them"
        );
    }
}
