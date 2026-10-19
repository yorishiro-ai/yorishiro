//! Licence state and the authentication override, composed after the base installer.

use std::sync::Arc;

use loco_rs::app::AppContext;

use crate::db::AppContextBackend;
use crate::edition::ee::controllers::middleware::{auth, edition::LicenceState};

pub(crate) fn install(ctx: &AppContext) {
    ctx.shared_store
        .insert(Arc::new(LicenceState::from_config(&ctx.config)));
    if ctx.is_sqlite() {
        tracing::warn!(
            "browsing the marketplace and publishing a template version are unavailable on SQLite \
             and return backend_unsupported (501). Point DATABASE_URL at PostgreSQL to use them"
        );
    } else {
        ctx.shared_store
            .insert(Arc::new(auth::TenantScopedAuthenticator)
                as Arc<dyn crate::controllers::middleware::auth::Authenticator>);
    }
}
