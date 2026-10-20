pub mod app;
pub mod controllers;
pub mod data;
pub mod db;
pub mod db_enum;
pub mod dtos;

// `time` owns the cross-cutting timezone policy used by application, DB, and queue boundaries.

#[cfg(feature = "enterprise")]
#[path = "../ee/mod.rs"]
pub mod ee;

/// Assembly stays in the crate root; base modules remain independent of the overlay.
pub mod edition {
    use std::path::Path;

    use loco_rs::prelude::async_trait;
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

    use crate::app::{self as base, RouteMounts};
    use crate::controllers::mcp::McpToolSet;
    use crate::workers::registry::WorkerRegistry;

    #[cfg(feature = "enterprise")]
    pub use crate::ee;
    #[cfg(not(feature = "enterprise"))]
    use community as active;
    #[cfg(feature = "enterprise")]
    use ee::app as active;

    pub struct App;

    /// The worker registry composed for this build.
    #[must_use]
    pub fn workers() -> WorkerRegistry {
        active::workers(WorkerRegistry::community())
    }

    #[async_trait]
    impl Hooks for App {
        fn app_name() -> &'static str {
            env!("CARGO_CRATE_NAME")
        }

        fn init_logger(ctx: &AppContext) -> Result<bool> {
            crate::initializers::logger::init::<Self>(ctx)
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
            base::boot::<Self>(mode, environment, config).await
        }

        async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
            Ok(base::initializers())
        }

        async fn before_run(ctx: &AppContext) -> Result<()> {
            base::before_run(ctx);
            Ok(())
        }

        async fn after_context(ctx: AppContext) -> Result<AppContext> {
            let ctx = base::after_context(ctx).await?;
            crate::initializers::timezone::prepare_cli(&ctx).await?;
            active::compose_context(&ctx)?;
            let settings = ctx.config.settings::<crate::data::settings::Settings>()?;
            let cap = active::tenant_creation_cap(settings.max_tenants, &ctx);
            ctx.shared_store
                .insert(crate::data::tenant_policy::TenantCap::new(cap));
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
            base::connect_workers(ctx, queue, &workers()).await
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

        async fn seed(ctx: &AppContext, base_path: &Path) -> Result<()> {
            active::seed(ctx).await?;
            base::seed(ctx, base_path).await
        }
    }

    #[cfg(not(feature = "enterprise"))]
    mod community {
        use loco_rs::{Result, app::AppContext, task::Tasks};

        use crate::app::RouteMounts;
        use crate::controllers::mcp::McpToolSet;
        use crate::workers::registry::WorkerRegistry;

        pub(super) fn compose_context(_ctx: &AppContext) -> Result<()> {
            Ok(())
        }

        pub(super) fn tenant_creation_cap(
            configured: i32,
            _ctx: &AppContext,
        ) -> impl Fn() -> Option<i32> + Send + Sync + 'static {
            if configured != 1 {
                tracing::warn!(
                    "YORISHIRO_MAX_TENANTS is ignored in Community Edition; enforcing one tenant"
                );
            }
            || Some(1)
        }

        pub(super) fn workers(registry: WorkerRegistry) -> WorkerRegistry {
            registry
        }
        pub(super) fn mount_routes(_mounts: &mut RouteMounts, _ctx: &AppContext) {}
        pub(super) fn mcp_tools(tools: McpToolSet) -> McpToolSet {
            tools
        }
        pub(super) fn register_tasks(_tasks: &mut Tasks) {}

        #[allow(clippy::unused_async)]
        pub(super) async fn seed(_ctx: &AppContext) -> Result<()> {
            Ok(())
        }
    }
}
pub mod error;
pub mod initializers;
pub mod models;
pub mod services;
pub mod tasks;
/// Shared application time policy, configured IANA zone, and wall-clock helpers.
pub mod time;
pub mod workers;

pub use edition::App;
pub use error::YorishiroError;
