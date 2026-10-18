//! Default authentication implementation installed before application extensions run.

use loco_rs::app::AppContext;

use crate::db::AppContextBackend;

pub(crate) fn install(ctx: &AppContext) {
    if ctx.is_postgres() {
        ctx.shared_store
            .insert(crate::controllers::middleware::auth::default_authenticator());
    }
}
