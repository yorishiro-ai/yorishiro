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
        // The embedding provider seam, read from shared_store the same way as DbHandle/
        // Authenticator above. Boot fails loudly if it's misconfigured (missing env vars, or a
        // provider the config can't build) rather than deferring the error to the first search.
        let embedding_provider =
            crate::services::embedding::build_embedding_provider().map_err(|e| {
                loco_rs::Error::Message(format!("failed to build embedding provider: {e}"))
            })?;
        ctx.shared_store.insert(embedding_provider);
        Ok(ctx)
    }

    fn routes(_ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes()
            .add_route(controllers::auth::routes())
            .add_route(controllers::entities::routes())
            .add_route(controllers::entities::migration_routes())
            .add_route(controllers::export::routes())
            .add_route(controllers::import::routes())
            .add_route(controllers::members::routes())
            .add_route(controllers::relations::routes())
            .add_route(controllers::schemas::routes())
            .add_route(controllers::schemas::template_routes())
            .add_route(controllers::search::routes())
            .add_route(controllers::setup::routes())
            .add_route(controllers::system::routes())
            .add_route(controllers::template_library::routes())
            .add_route(controllers::whoami::routes())
            .add_route(controllers::workspaces::routes())
    }

    /// Mounts the MCP server under `/mcp` and layers the maintenance guard and the auth rate
    /// limiter over everything, REST and MCP alike. `rmcp`'s `StreamableHttpService` is a plain
    /// `tower::Service`, not something `Hooks::routes()`/`AppRoutes` can carry, so it's mounted
    /// here instead: this hook runs after Loco's own routes are built, which is where Loco
    /// itself says custom Axum logic belongs. The middleware layers must come after the MCP
    /// mount, not before: `.layer` wraps everything already on the router at the point it's
    /// called.
    async fn after_routes(router: axum::Router, ctx: &AppContext) -> Result<axum::Router> {
        let router = controllers::mcp::mount(router, ctx);
        let rate_limiter =
            std::sync::Arc::new(crate::services::rate_limit::RateLimiter::from_env());
        let router = router.layer(axum::middleware::from_fn_with_state(
            rate_limiter,
            crate::services::rate_limit::enforce,
        ));
        Ok(router.layer(axum::middleware::from_fn_with_state(
            ctx.clone(),
            crate::services::maintenance::maintenance_guard,
        )))
    }

    async fn connect_workers(_ctx: &AppContext, _queue: &Queue) -> Result<()> {
        Ok(())
    }

    #[allow(unused_variables)]
    fn register_tasks(tasks: &mut Tasks) {
        tasks.register(tasks::create_tenant::CreateTenant);
        tasks.register(tasks::create_workspace::CreateWorkspace);
        tasks.register(tasks::create_api_key::CreateApiKey);
        tasks.register(tasks::create_invite::CreateInvite);
        tasks.register(tasks::list_tenants::ListTenants);
        tasks.register(tasks::list_workspaces::ListWorkspaces);
        tasks.register(tasks::create_user::CreateUser);
        tasks.register(tasks::add_member::AddMember);
        tasks.register(tasks::list_members::ListMembers);
        tasks.register(tasks::list_api_keys::ListApiKeys);
        tasks.register(tasks::revoke_api_key::RevokeApiKey);
        tasks.register(tasks::resync_embeddings::ResyncEmbeddings);
        tasks.register(tasks::maintenance::Maintenance);
        tasks.register(tasks::maintenance_status::MaintenanceStatus);
        // tasks-inject (do not remove)
    }
    async fn truncate(_ctx: &AppContext) -> Result<()> {
        Ok(())
    }
    async fn seed(_ctx: &AppContext, _base: &Path) -> Result<()> {
        Ok(())
    }
}
