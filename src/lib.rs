//! The Yorishiro application: a Loco app with thin entry points, model-owned logic and background workers.
//! This file is the only place that assembles the enterprise overlay and implements `Hooks`.

pub mod app;
pub mod controllers;
pub mod data;
pub mod db;
pub mod dtos;

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

    pub(crate) async fn quota_override(
        ctx: &AppContext,
        tenant_id: uuid::Uuid,
    ) -> std::result::Result<Option<crate::data::quotas::Quotas>, crate::YorishiroError> {
        active::quota_override(ctx, tenant_id).await
    }

    /// The worker registry composed for this build.
    #[must_use]
    pub fn workers() -> WorkerRegistry {
        active::workers(WorkerRegistry::community())
    }

    pub(crate) async fn workspace_worker_class(
        conn: &sea_orm::DatabaseConnection,
        workspace_id: uuid::Uuid,
    ) -> std::result::Result<
        Option<crate::workers::embedding_sync::WorkerClass>,
        crate::YorishiroError,
    > {
        active::workspace_worker_class(conn, workspace_id).await
    }

    pub(crate) async fn workspace_embedding_provider(
        conn: &sea_orm::DatabaseConnection,
        workspace_id: uuid::Uuid,
    ) -> std::result::Result<
        Option<std::sync::Arc<dyn crate::services::embedding::EmbeddingProvider>>,
        crate::YorishiroError,
    > {
        active::workspace_embedding_provider(conn, workspace_id).await
    }

    pub(crate) async fn workspace_embedding_metadata(
        conn: &sea_orm::DatabaseConnection,
        workspace_id: uuid::Uuid,
    ) -> std::result::Result<
        Option<crate::services::embedding::EmbeddingMetadata>,
        crate::YorishiroError,
    > {
        active::workspace_embedding_metadata(conn, workspace_id).await
    }

    #[async_trait]
    impl Hooks for App {
        fn app_name() -> &'static str {
            env!("CARGO_CRATE_NAME")
        }

        async fn load_config(env: &Environment) -> Result<Config> {
            base::load_config(env)
        }

        fn app_version() -> String {
            base::app_version()
        }

        async fn boot(
            mode: StartMode,
            environment: &Environment,
            config: Config,
        ) -> Result<BootResult> {
            Box::pin(base::boot::<Self>(mode, environment, config)).await
        }

        async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
            Ok(base::initializers())
        }

        async fn before_run(ctx: &AppContext) -> Result<()> {
            base::before_run(ctx);
            Ok(())
        }

        async fn after_context(ctx: AppContext) -> Result<AppContext> {
            let mut ctx = base::after_context(ctx).await?;
            active::compose_context(&ctx)?;
            let schedule = active::reindex_schedule(&ctx)?;
            crate::tasks::reindex_scheduler::configure(&mut ctx.config, schedule.as_deref());
            let settings = crate::data::settings::configured(&ctx)?;
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

        async fn seed(ctx: &AppContext, _base_path: &Path) -> Result<()> {
            active::seed(ctx).await
        }
    }

    #[cfg(not(feature = "enterprise"))]
    mod community {
        use loco_rs::{Result, app::AppContext, task::Tasks};

        use crate::app::RouteMounts;
        use crate::controllers::mcp::McpToolSet;
        use crate::workers::registry::WorkerRegistry;

        #[expect(
            clippy::unused_async,
            reason = "edition contributions share an async quota lookup contract"
        )]
        pub(super) async fn quota_override(
            _ctx: &AppContext,
            _tenant_id: uuid::Uuid,
        ) -> std::result::Result<Option<crate::data::quotas::Quotas>, crate::YorishiroError>
        {
            Ok(None)
        }

        #[expect(
            clippy::unused_async,
            reason = "edition contributions share an async workspace lookup contract"
        )]
        pub(super) async fn workspace_worker_class(
            _conn: &sea_orm::DatabaseConnection,
            _workspace_id: uuid::Uuid,
        ) -> std::result::Result<
            Option<crate::workers::embedding_sync::WorkerClass>,
            crate::YorishiroError,
        > {
            Ok(None)
        }

        #[expect(
            clippy::unused_async,
            reason = "edition contributions share an async provider lookup contract"
        )]
        pub(super) async fn workspace_embedding_provider(
            _conn: &sea_orm::DatabaseConnection,
            _workspace_id: uuid::Uuid,
        ) -> std::result::Result<
            Option<std::sync::Arc<dyn crate::services::embedding::EmbeddingProvider>>,
            crate::YorishiroError,
        > {
            Ok(None)
        }

        #[expect(
            clippy::unused_async,
            reason = "edition contributions share an async metadata lookup contract"
        )]
        pub(super) async fn workspace_embedding_metadata(
            _conn: &sea_orm::DatabaseConnection,
            _workspace_id: uuid::Uuid,
        ) -> std::result::Result<
            Option<crate::services::embedding::EmbeddingMetadata>,
            crate::YorishiroError,
        > {
            Ok(None)
        }

        #[expect(
            clippy::unnecessary_wraps,
            reason = "the overlay validates its schedule configuration"
        )]
        pub(super) const fn reindex_schedule(_ctx: &AppContext) -> Result<Option<String>> {
            Ok(None)
        }

        #[expect(
            clippy::unnecessary_wraps,
            reason = "edition context composition shares a fallible contract"
        )]
        pub(super) const fn compose_context(_ctx: &AppContext) -> Result<()> {
            Ok(())
        }

        pub(super) fn tenant_creation_cap(
            configured: i32,
            _ctx: &AppContext,
        ) -> impl Fn() -> Option<i32> + Send + Sync + 'static {
            use crate::data::tenant_policy::DEFAULT_TENANT_CAP;
            if configured != DEFAULT_TENANT_CAP {
                tracing::warn!(
                    "YORISHIRO_MAX_TENANTS is ignored in Community Edition; enforcing one tenant"
                );
            }
            || Some(DEFAULT_TENANT_CAP)
        }

        pub(super) const fn workers(registry: WorkerRegistry) -> WorkerRegistry {
            registry
        }
        pub(super) const fn mount_routes(_mounts: &mut RouteMounts, _ctx: &AppContext) {}
        pub(super) const fn mcp_tools(tools: McpToolSet) -> McpToolSet {
            tools
        }
        pub(super) const fn register_tasks(_tasks: &mut Tasks) {}

        #[expect(clippy::unused_async, reason = "Loco declares this hook async")]
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
pub mod workers;

pub use edition::App;
pub(crate) use edition::{
    quota_override, workspace_embedding_metadata, workspace_embedding_provider,
    workspace_worker_class,
};
pub use error::YorishiroError;
