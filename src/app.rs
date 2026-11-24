//! The base application's wiring, one function per Loco `Hooks` method.
//!
//! This crate has exactly one `Hooks` implementation, and it lives in `crate::edition`.
//! That implementation calls the functions below for the base behaviour and adds whatever its edition contributes.
//! Nothing here knows which editions exist: the types this module hands out (`RouteMounts`, [`WorkerRegistry`], [`McpToolSet`]) are how an edition adds to the base.

use loco_rs::{
    Result,
    app::{AppContext, Hooks, Initializer},
    boot::{BootResult, StartMode, create_app},
    config::Config,
    controller::AppRoutes,
    environment::Environment,
    task::Tasks,
};
use migration::Migrator;

use crate::controllers;
use crate::controllers::mcp::McpToolSet;
use crate::initializers;
use crate::tasks;
use crate::workers::registry::WorkerRegistry;

pub(crate) fn app_version() -> String {
    format!(
        "{} ({})",
        env!("CARGO_PKG_VERSION"),
        option_env!("BUILD_SHA")
            .or(option_env!("GITHUB_SHA"))
            .unwrap_or("dev")
    )
}

/// Loads the configuration, then applies what the command line asks of the workers it starts.
///
/// `yorishiro start` does not go through `Hooks::boot`, so this is the first point on its path that can still refuse a worker start before anything is built, and the last that can still change the queue configuration the provider is created from.
///
/// # Errors
/// Returns an error when the configuration is invalid, or when the command starts workers without a valid worker target.
pub(crate) fn load_config(env: &Environment) -> Result<Config> {
    let mut config = crate::data::config::load(env)?;
    let start = crate::workers::start::worker_start_from_args(std::env::args_os())
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    crate::workers::start::configure_start(&mut config, start);
    Ok(config)
}

/// Validates the worker targets, then boots through Loco as `H`.
///
/// The configuration was validated when it was loaded (see [`load_config`]).
/// The command line does not call this; a caller that boots with a [`StartMode`], such as the test harness, does, and gets the same worker-target checks.
///
/// # Errors
/// Returns an error when the mode starts workers without a valid worker target.
pub(crate) async fn boot<H: Hooks>(
    mode: StartMode,
    environment: &Environment,
    mut config: Config,
) -> Result<BootResult> {
    crate::workers::start::configure(&mut config, &mode)?;
    // Register sqlite-vec for the test harness path (the test binary never runs main.rs).
    // The call site in main.rs already covers all CLI subcommands.
    crate::db::register_sqlite_extensions();
    create_app::<H, Migrator>(mode, environment, config).await
}

pub(crate) fn before_run(ctx: &AppContext) {
    for advisory in crate::data::config::topology_advisories(&ctx.config) {
        tracing::warn!("{advisory}");
    }
}

pub(crate) fn initializers() -> Vec<Box<dyn Initializer>> {
    vec![Box::new(initializers::db_load_guard::LoadGuard)]
}

/// Builds the base pools and shared-store seams.
///
/// An edition that brings its own rule for a seam inserts its own `Arc<dyn Trait>` afterwards: the store is keyed by `TypeId`, so the later insert wins without changing any call site.
pub(crate) async fn after_context(ctx: AppContext) -> Result<AppContext> {
    let mut ctx = ctx;
    initializers::database::install(&mut ctx).await?;
    let ctx = install_services(ctx)?;
    initializers::queue::install(&ctx);
    Ok(ctx)
}

/// The route tree under construction, together with the `OpenAPI` document its routes generated.
///
/// An edition mounts its own route groups after the base ones, so it adds paths rather than replacing any.
pub(crate) struct RouteMounts {
    app_routes: AppRoutes,
    api: aide::openapi::OpenApi,
    credential_paths: Vec<&'static str>,
}

/// The unauthenticated credential paths the per-IP rate limit applies to, published by [`RouteMounts::finish`].
#[derive(Clone)]
struct CredentialPaths(Vec<&'static str>);

impl RouteMounts {
    /// The base route groups, mounted.
    pub(crate) fn community() -> Self {
        let mut mounts = Self {
            app_routes: AppRoutes::with_default_routes(),
            api: aide::openapi::OpenApi::default(),
            credential_paths: Vec::new(),
        };
        mounts
            .guard_credentials("/auth/signup")
            .guard_credentials("/auth/root/login")
            .guard_credentials("/auth/tenant/login");

        mounts.mount(controllers::audit_log::routes());
        mounts.mount(controllers::api_keys::routes());
        mounts.mount(controllers::auth::routes());
        mounts.mount(controllers::entities::routes());
        mounts.mount(controllers::entities::migration_routes());
        mounts.mount(controllers::export::routes());
        mounts.mount(controllers::import::routes());
        mounts.mount(controllers::iam::routes());
        mounts.mount(controllers::members::routes());
        mounts.mount(controllers::tenants::routes());
        mounts.mount(controllers::relations::routes());
        mounts.mount(controllers::schemas::routes());
        mounts.mount(controllers::schemas::template_routes());
        mounts.mount(controllers::search::routes());
        mounts.mount(controllers::setup::routes());
        mounts.mount(controllers::template_library::routes());
        mounts.mount(controllers::whoami::routes());
        mounts.mount(controllers::users::routes());
        mounts.mount(controllers::workspaces::routes());
        mounts
    }

    /// Registers a route group with Loco and merges its generated operations into the document.
    pub(crate) fn mount(&mut self, routes: controllers::openapi::Routes) -> &mut Self {
        let controllers::openapi::Routes {
            loco: routes, api, ..
        } = routes;
        controllers::openapi::merge_api(&mut self.api, api);
        let current = std::mem::replace(&mut self.app_routes, AppRoutes::empty());
        self.app_routes = current.add_route(routes);
        self
    }

    /// Applies the per-IP credential rate limit to `path`, which must be reachable without a bearer token.
    pub(crate) fn guard_credentials(&mut self, path: &'static str) -> &mut Self {
        self.credential_paths.push(path);
        self
    }

    /// Publishes the document and credential paths to the shared store, and returns the route tree.
    pub(crate) fn finish(mut self, ctx: &AppContext) -> AppRoutes {
        controllers::openapi::assert_unique_operation_ids(&mut self.api);
        ctx.shared_store.insert(self.api);
        ctx.shared_store
            .insert(CredentialPaths(self.credential_paths));
        self.app_routes
    }
}

/// Mounts the MCP server under `/mcp` and the swagger docs.
///
/// Rate limiting and the maintenance guard are layered here because `/mcp` is mounted here: `Hooks::routes` layers would not cover it.
/// The `server.middlewares:` config block is used only for Loco's built-in middleware (`request_id`, `logger`, and shared REST/MCP CORS).
///
/// `rmcp`'s `StreamableHttpService` is a plain `tower::Service`, not
/// something `Hooks::routes()`/`AppRoutes` can carry, so it's mounted here
/// instead: this hook runs after Loco's own routes are built, which is where
/// Loco itself says custom Axum logic belongs.
pub(crate) fn after_routes(
    router: axum::Router,
    ctx: &AppContext,
    tools: McpToolSet,
) -> Result<axum::Router> {
    let api = ctx
        .shared_store
        .get::<aide::openapi::OpenApi>()
        .ok_or_else(|| loco_rs::Error::Message("OpenAPI document missing".into()))?;
    let router = controllers::openapi::swagger::mount(router, &api);
    let router = controllers::mcp::mount(router, ctx, move |ctx| tools.server(ctx));
    let credential_paths = ctx
        .shared_store
        .get::<CredentialPaths>()
        .ok_or_else(|| loco_rs::Error::Message("credential paths missing".into()))?;
    let router = initializers::rate_limit::after_routes(router, ctx, &credential_paths.0)?;
    let router = if let Some(cors) = ctx
        .config
        .server
        .middlewares
        .cors
        .as_ref()
        .filter(|cors| cors.enable)
    {
        router.layer(cors.cors()?)
    } else {
        router
    };
    let router = router.layer(axum::middleware::from_fn_with_state(
        ctx.clone(),
        controllers::middleware::maintenance::maintenance_guard,
    ));
    let settings = ctx.config.settings::<crate::data::settings::Settings>()?;
    if settings.http_allowed_networks.is_empty() {
        return Ok(router);
    }
    Ok(router.layer(axum::middleware::from_fn_with_state(
        std::sync::Arc::<[ipnet::IpNet]>::from(settings.http_allowed_networks),
        controllers::middleware::client_network::require_allowed_network,
    )))
}

/// Enables Loco's built-in middleware stack (`request_id`, `logger`, etc.).
/// Custom middleware (rate limiter, maintenance guard) stays in `after_routes` so it also covers the `/mcp` mount.
pub(crate) fn middlewares(
    ctx: &AppContext,
) -> Vec<Box<dyn loco_rs::controller::middleware::MiddlewareLayer>> {
    use loco_rs::controller::middleware::MiddlewareStackExt;

    let logger_config = ctx
        .config
        .server
        .middlewares
        .logger
        .clone()
        .unwrap_or(loco_rs::controller::middleware::logger::Config { enable: true });
    let mut stack = loco_rs::controller::middleware::default_middleware_stack(ctx);
    // Apply CORS after mounting MCP so REST and MCP share the same layer.
    stack.retain(|middleware| middleware.name() != "cors");
    stack.replace(
        "logger",
        Box::new(controllers::middleware::access_log::Middleware::new(
            &logger_config,
            &ctx.environment,
        )),
    );
    stack
}

/// Gives a queue-consuming process what only it needs, then registers its workers.
///
/// Loco calls this only for start modes that run workers, and only on the second of the two contexts `start` builds, so the provider is built once per worker process.
/// The check keeps that true if a caller ever connects twice.
/// With the provider and workers in place, the startup reindex scan runs and a periodic recovery task starts.
///
/// # Errors
/// Returns an error when the configured provider cannot be built, so a worker with a misconfigured model fails at start rather than on its first job, or when Loco refuses a registration.
pub(crate) async fn connect_workers(
    ctx: &AppContext,
    queue: &loco_rs::bgworker::Queue,
    workers: &WorkerRegistry,
) -> Result<()> {
    let role = crate::data::settings::configured(ctx)?.consumer_role;
    if role == crate::data::settings::ConsumerRole::Embedding {
        initializers::embedding::install_provider(ctx).await?;
    }
    workers.connect(ctx, queue).await?;
    initializers::queue::start_relay(ctx)?;
    if role == crate::data::settings::ConsumerRole::Embedding {
        crate::workers::startup_reindex::run(ctx).await;
    }
    Ok(())
}

pub(crate) fn register_tasks(tasks: &mut Tasks) {
    tasks.register(tasks::inspect_tenant_retirement::InspectTenantRetirement);
    tasks.register(tasks::disable_tenant::DisableTenant);
    tasks.register(tasks::delete_tenant::DeleteTenant);
    tasks.register(tasks::delete_due_tenants::DeleteDueTenants);
    tasks.register(tasks::reset_root_password::ResetRootPassword);
    // tasks-inject
    tasks.register(tasks::delete_user::DeleteUser);
    tasks.register(tasks::resync_embeddings::ResyncEmbeddings);
    tasks.register(tasks::embedding_index_maintenance::EmbeddingIndexMaintenance);
    tasks.register(tasks::maintenance::Maintenance);
    tasks.register(tasks::maintenance_status::MaintenanceStatus);
    tasks.register(tasks::recover_worker_dispatches::RecoverWorkerDispatches);
    tasks.register(tasks::tenant_reindex_scheduler::TenantReindexScheduler);
    tasks.register(tasks::db_load_guard::DbLoadGuard);
}

pub(crate) async fn on_shutdown(ctx: &AppContext) {
    initializers::queue::shutdown(ctx).await;
    initializers::db_load_guard::shutdown(ctx).await;
    // Loco drains queue workers after this hook; their contexts keep the pools alive.
}

/// Builds the pools and shared services every entry point, tasks included, depends on.
///
/// The embedding provider is deliberately not among them: it can hold a model of a gigabyte or more, and the API and MCP server, the scheduler, and every CLI command run without one.
/// Only a process that consumes the queue installs it, in [`connect_workers`].
/// Loco builds a context twice for `yorishiro start`, so anything built here would be built twice.
fn install_services(ctx: AppContext) -> Result<AppContext> {
    let settings = ctx.config.settings::<crate::data::settings::Settings>()?;
    // The pools install before this hook and already parse the zone; `config::validate` accepted it at load.
    let zone = crate::data::time::parse_timezone(&settings.timezone).map_err(|error| {
        loco_rs::Error::Message(format!(
            "settings.timezone '{}': {error}",
            settings.timezone
        ))
    })?;
    initializers::auth::install(&ctx);
    initializers::embedding::install(&ctx, &settings);
    initializers::rate_limit::install(&ctx, &settings);
    // One zone parse per boot: per-record timestamp rendering takes the installed copy.
    ctx.shared_store.insert(zone);
    Ok(ctx)
}
