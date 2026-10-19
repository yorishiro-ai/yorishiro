//! Database startup wiring; connection primitives remain in `crate::db`.

use loco_rs::{Result, app::AppContext};

use crate::db::{self, AppContextBackend};

pub(crate) async fn install(ctx: &mut AppContext) -> Result<()> {
    if ctx.is_sqlite() {
        db::require_min_sqlite_connections(ctx.config.database.max_connections)
            .map_err(loco_rs::Error::Message)?;
        db::replace_sqlite_app_pool(ctx).await?;
    } else if ctx.is_postgres() {
        db::replace_postgres_app_pool(ctx).await?;
        db::install_pools(ctx).await?;
    }
    Ok(())
}
