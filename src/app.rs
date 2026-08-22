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

#[allow(unused_imports)]
use crate::{controllers, tasks};

pub struct App;
#[async_trait]
impl Hooks for App {
    fn app_name() -> &'static str {
        env!("CARGO_CRATE_NAME")
    }

    fn app_version() -> String {
        format!(
            "{} ({})",
            env!("CARGO_PKG_VERSION"),
            option_env!("BUILD_SHA")
                .or(option_env!("GITHUB_SHA"))
                .unwrap_or("dev")
        )
    }

    async fn boot(
        mode: StartMode,
        environment: &Environment,
        config: Config,
    ) -> Result<BootResult> {
        create_app::<Self, Migrator>(mode, environment, config).await
    }

    async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
        Ok(vec![])
    }

    /// Builds the RLS-aware raw sqlx pool and stores it in `shared_store`.
    ///
    /// Loco's own database connection (`ctx.db`, a `sea_orm::DatabaseConnection`) is built from
    /// `sea_orm::ConnectOptions`, which has no `after_connect`/`after_release` hook. This
    /// deployment's row-level security depends on a `SET ROLE` per physical connection and
    /// `set_config(...)` per request, so that lifecycle is built separately here, on a
    /// hand-constructed `sqlx::PgPool`, and stored for handlers to retrieve via
    /// `ctx.shared_store.get_ref::<crate::db::DbHandle>()`. See
    /// <https://github.com/yotsunagi/yorishiro/issues/221>.
    async fn after_context(ctx: AppContext) -> Result<AppContext> {
        let database_url = ctx.config.database.uri.clone();
        let tenant =
            crate::db::TenantDb::connect(&database_url, ctx.config.database.max_connections)
                .await
                .map_err(|e| {
                    loco_rs::Error::Message(format!("failed to build tenant pool: {e}"))
                })?;
        // The identity pool connects as the migration role (bypassing RLS) for control-plane
        // access: signup, setup, the admin CLI. It's a plain pool with no after_connect/
        // after_release, since it never scopes to a tenant/workspace.
        let identity = sqlx::postgres::PgPoolOptions::new()
            .max_connections(ctx.config.database.max_connections)
            .connect(&database_url)
            .await
            .map_err(|e| loco_rs::Error::Message(format!("failed to build identity pool: {e}")))?;
        ctx.shared_store
            .insert(crate::db::DbHandle { tenant, identity });
        // The authenticator seam: a deployment that needs a different authentication rule
        // (a key naming its workspace per request, an external identity system) replaces this
        // insert with its own `Arc<dyn Authenticator>` rather than changing every call site.
        ctx.shared_store
            .insert(crate::services::auth::default_authenticator());
        Ok(ctx)
    }

    fn routes(_ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes()
            .add_route(controllers::entities::routes())
            .add_route(controllers::export::routes())
            .add_route(controllers::import::routes())
            .add_route(controllers::relations::routes())
            .add_route(controllers::schemas::routes())
    }
    async fn connect_workers(_ctx: &AppContext, _queue: &Queue) -> Result<()> {
        Ok(())
    }

    #[allow(unused_variables)]
    fn register_tasks(tasks: &mut Tasks) {
        tasks.register(tasks::create_tenant::CreateTenant);
        tasks.register(tasks::create_workspace::CreateWorkspace);
        tasks.register(tasks::create_api_key::CreateApiKey);
        // tasks-inject (do not remove)
    }
    async fn truncate(_ctx: &AppContext) -> Result<()> {
        Ok(())
    }
    async fn seed(_ctx: &AppContext, _base: &Path) -> Result<()> {
        Ok(())
    }
}
