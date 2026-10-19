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
    if cli_requires_timezone_gate(std::env::args_os()) {
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

/// Identifies CLI commands that execute application work before Loco's `before_run` hook.
///
/// `shortcut: Does not parse arbitrary subcommand option grammars; expand this if Loco changes its global CLI flags.`
pub fn cli_requires_timezone_gate<I, T>(args: I) -> bool
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString>,
{
    let mut args = args.into_iter().map(Into::into).skip(1);
    while let Some(raw) = args.next() {
        let arg = raw.to_string_lossy();
        if arg == "-e" || arg == "--environment" {
            args.next();
            continue;
        }
        if arg.starts_with("--environment=") || (arg.starts_with("-e") && arg.len() > 2) {
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        return matches!(arg.as_ref(), "task" | "t" | "scheduler" | "jobs");
    }
    false
}

/// Checks whether a work command may proceed with the persisted active timezone.
///
/// # Errors
/// Returns an error when a gated command has no persisted timezone state.
pub fn cli_timezone_gate_result<I, T>(args: I, active_timezone: Option<&str>) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString>,
{
    if cli_requires_timezone_gate(args) && active_timezone.is_none() {
        return Err(loco_rs::Error::Message(
            "timezone_state singleton missing; run `db migrate` before executing tasks or workers"
                .into(),
        ));
    }
    Ok(())
}
