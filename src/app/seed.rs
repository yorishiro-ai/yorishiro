use std::path::Path;

use loco_rs::{Result, app::AppContext};

pub(super) async fn community(ctx: &AppContext, base: &Path) -> Result<()> {
    let fixtures = base.join("fixtures");
    if fixtures.join("tenant_tenants.yaml").exists() {
        loco_rs::db::seed::<crate::models::tenant_tenants::ActiveModel>(
            &ctx.db,
            &fixtures.join("tenant_tenants.yaml").display().to_string(),
        )
        .await?;
    }
    if fixtures.join("workspaces.yaml").exists() {
        loco_rs::db::seed::<crate::models::workspace_workspaces::ActiveModel>(
            &ctx.db,
            &fixtures.join("workspaces.yaml").display().to_string(),
        )
        .await?;
    }
    Ok(())
}
