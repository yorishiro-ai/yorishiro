//! Default authentication implementation installed before application extensions run.

use loco_rs::app::AppContext;

pub(crate) fn install(ctx: &AppContext) {
    ctx.shared_store
        .insert(crate::controllers::middleware::auth::default_authenticator());
}
