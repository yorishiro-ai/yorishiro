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
