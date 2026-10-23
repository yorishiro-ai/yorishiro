//! Database startup wiring; connection primitives remain in `crate::db`.

use loco_rs::{Result, app::AppContext};

use crate::db::{self, AppContextBackend};

pub(crate) async fn install(ctx: &mut AppContext) -> Result<()> {
    db::replace_app_pool(ctx).await?;
    if ctx.is_postgres() {
        db::install_pools(ctx).await?;
    }
    Ok(())
}
