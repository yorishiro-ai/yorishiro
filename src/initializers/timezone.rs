//! Timezone activation for server/worker startup and CLI commands.
//!
//! Context replacement remains in after_context, while activation must follow migrations.
//! CLI work commands do not run Loco's initializer lifecycle, so they call the same activation directly.

use loco_rs::{
    Result,
    app::{AppContext, Initializer},
    prelude::async_trait,
};

pub(crate) struct Timezone;

#[async_trait]
impl Initializer for Timezone {
    fn name(&self) -> String {
        "timezone".into()
    }

    async fn before_run(&self, ctx: &AppContext) -> Result<()> {
        activate(ctx).await
    }
}

pub(crate) async fn prepare_cli(ctx: &AppContext) -> Result<()> {
    if crate::app::cli_requires_timezone_gate(std::env::args_os()) {
        activate(ctx).await?;
    }
    Ok(())
}

async fn activate(ctx: &AppContext) -> Result<()> {
    crate::models::timezone_state::activate(
        &ctx.db,
        &ctx.config
            .settings::<crate::data::settings::Settings>()?
            .timezone,
    )
    .await
}
