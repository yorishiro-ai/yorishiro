//! Database startup wiring; connection primitives remain in `crate::db`.

use loco_rs::{Result, app::AppContext};

use crate::db;

pub(crate) async fn install(ctx: &AppContext) -> Result<()> {
    if db::is_sqlite(&ctx.db) {
        db::require_min_sqlite_connections(ctx.config.database.max_connections)
            .map_err(loco_rs::Error::Message)?;
    }
    if db::is_postgres(&ctx.db) {
        db::install_pools(ctx).await?;
    }
    Ok(())
}
