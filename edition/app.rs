//! The crate's only `Hooks` implementation.

use std::path::Path;

use async_trait::async_trait;
use loco_rs::{
    Result,
    app::{AppContext, Hooks, Initializer},
    bgworker::Queue,
    boot::{BootResult, StartMode},
    config::Config,
    controller::AppRoutes,
    environment::Environment,
    task::Tasks,
};

use super::active;
use crate::app::{self as base, RouteMounts};
use crate::controllers::mcp::McpToolSet;
use crate::workers::registry::WorkerRegistry;

pub struct App;

/// Every worker this build registers: the base's, then the active edition's.
///
/// Loco registration, the Redis queue configuration and [`worker_tags`] all read this, so they cannot disagree.
#[must_use]
pub fn workers() -> WorkerRegistry {
    active::workers(WorkerRegistry::community())
}

/// Every tag `yorishiro start --worker=...` can be asked for, as printed by `yorishiro worker-tags`.
#[must_use]
pub fn worker_tags() -> Vec<String> {
    workers().tags()
}

#[async_trait]
impl Hooks for App {
    fn app_name() -> &'static str {
        env!("CARGO_CRATE_NAME")
    }

    async fn load_config(env: &Environment) -> Result<Config> {
        base::load_config(env).await
    }

    fn app_version() -> String {
        base::app_version()
    }

    async fn boot(
        mode: StartMode,
        environment: &Environment,
        config: Config,
    ) -> Result<BootResult> {
        base::boot::<Self>(mode, environment, config, &workers()).await
    }

    async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
        Ok(base::initializers())
    }

    async fn after_context(ctx: AppContext) -> Result<AppContext> {
        let ctx = base::after_context(ctx).await?;
        active::compose_context(&ctx);
        Ok(ctx)
    }

    fn routes(ctx: &AppContext) -> AppRoutes {
        let mut mounts = RouteMounts::community();
        active::mount_routes(&mut mounts, ctx);
        mounts.finish(ctx)
    }

    async fn after_routes(router: axum::Router, ctx: &AppContext) -> Result<axum::Router> {
        base::after_routes(router, ctx, active::mcp_tools(McpToolSet::community()))
    }

    fn middlewares(
        ctx: &AppContext,
    ) -> Vec<Box<dyn loco_rs::controller::middleware::MiddlewareLayer>> {
        base::middlewares(ctx)
    }

    async fn connect_workers(ctx: &AppContext, queue: &Queue) -> Result<()> {
        workers().connect(ctx, queue).await
    }

    fn register_tasks(tasks: &mut Tasks) {
        base::register_tasks(tasks);
        active::register_tasks(tasks);
    }

    async fn on_shutdown(ctx: &AppContext) {
        base::on_shutdown(ctx).await;
    }

    async fn truncate(_ctx: &AppContext) -> Result<()> {
        Ok(())
    }

    /// The active edition seeds what it owns first, then the base seeds the demo tenant and workspace from `fixtures/`.
    async fn seed(ctx: &AppContext, base_path: &Path) -> Result<()> {
        active::seed(ctx).await?;
        base::seed(ctx, base_path).await
    }
}
