//! The paid edition's composition seam.
//!
//! `HostedApp` is a second `loco_rs::app::Hooks` implementation, distinct from
//! `yorishiro_core::app::App`, so `cli::main::<HostedApp, Migrator>()` boots the same
//! application with `ee/`'s pieces layered on. Every method delegates to `App`'s associated fn
//! for the base behaviour first; `ee/`-only wiring (the licence gate, and later the paid
//! routes/tasks/SPA) is added around that call rather than duplicating it, so a change to base's
//! boot sequence only needs updating here if this crate actually diverges from it.
//!
//! `yorishiro-core` must never depend on this crate; this crate depends on it. See CLAUDE.md's
//! "Editions and the `ee/` boundary" for the standing rule this file exists to satisfy.

pub mod services;

use async_trait::async_trait;
use loco_rs::{
    Result,
    app::{AppContext, Hooks, Initializer},
    bgworker::Queue,
    boot::{BootResult, StartMode, create_app},
    config::Config,
    controller::AppRoutes,
    environment::Environment,
    task::Tasks,
};
use migration::Migrator;
use std::path::Path;
use yorishiro_core::app::App;

use services::licence::LicenceState;

pub struct HostedApp;

#[async_trait]
impl Hooks for HostedApp {
    fn app_name() -> &'static str {
        env!("CARGO_CRATE_NAME")
    }

    fn app_version() -> String {
        App::app_version()
    }

    async fn boot(
        mode: StartMode,
        environment: &Environment,
        config: Config,
    ) -> Result<BootResult> {
        create_app::<Self, Migrator>(mode, environment, config).await
    }

    async fn initializers(ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
        App::initializers(ctx).await
    }

    /// Runs base's own `after_context` (the RLS-aware pool, the authenticator seam) first, then
    /// resolves the licence key and stores it in `shared_store` for gated paid handlers to read.
    /// An absent or invalid key does not fail boot: the free half must keep working with no
    /// licence configured, per `services::licence`'s own doc comment.
    async fn after_context(ctx: AppContext) -> Result<AppContext> {
        let ctx = App::after_context(ctx).await?;
        ctx.shared_store.insert(LicenceState::from_env());
        Ok(ctx)
    }

    fn routes(ctx: &AppContext) -> AppRoutes {
        App::routes(ctx)
    }

    async fn after_routes(router: axum::Router, ctx: &AppContext) -> Result<axum::Router> {
        App::after_routes(router, ctx).await
    }

    async fn connect_workers(ctx: &AppContext, queue: &Queue) -> Result<()> {
        App::connect_workers(ctx, queue).await
    }

    fn register_tasks(tasks: &mut Tasks) {
        App::register_tasks(tasks);
    }

    async fn truncate(ctx: &AppContext) -> Result<()> {
        App::truncate(ctx).await
    }

    async fn seed(ctx: &AppContext, base: &Path) -> Result<()> {
        App::seed(ctx, base).await
    }
}
