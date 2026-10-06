//! The base application's wiring, one function per Loco `Hooks` method.
//!
//! This crate has exactly one `Hooks` implementation, and it lives in `crate::edition`.
//! That implementation calls the functions below for the base behaviour and adds whatever its edition contributes.
//! Nothing here knows which editions exist: the types this module hands out ([`RouteMounts`], [`WorkerRegistry`], [`McpToolSet`]) are how an edition adds to the base.

use loco_rs::{
    Result,
    app::{AppContext, Hooks, Initializer},
    boot::{BootResult, StartMode, create_app},
    config::Config,
    controller::{AppRoutes, Routes},
    environment::Environment,
    task::Tasks,
};
use migration::Migrator;
use std::path::Path;
use std::sync::Arc;

use crate::controllers;
use crate::controllers::mcp::McpToolSet;
use crate::controllers::route_inventory::{RouteClass, RouteInventory};
use crate::db::AppContextBackend;
use crate::initializers;
use crate::tasks;
use crate::workers::dispatch::{EmbeddingSyncDispatcher, LocoJobDispatcher, ReindexDispatcher};
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

/// Loads the canonical plain-YAML configuration.
pub(crate) async fn load_config(env: &Environment) -> Result<Config> {
    crate::data::config::load(env).await
}

/// Validates the queue policy, makes a Redis queue poll every queue `workers` enqueue to, then boots through Loco as `H`.
pub(crate) async fn boot<H: Hooks>(
    mode: StartMode,
    environment: &Environment,
    mut config: Config,
    workers: &WorkerRegistry,
) -> Result<BootResult> {
    crate::data::config::validate_queue_policy(&config)?;
    crate::data::config::serve_worker_queues(&mut config, &workers.queues());
    // Register sqlite-vec for the test harness path (the test binary never runs main.rs).
    // The call site in main.rs already covers all CLI subcommands.
    crate::db::register_sqlite_extensions();
    create_app::<H, Migrator>(mode, environment, config).await
}

pub(crate) fn initializers() -> Vec<Box<dyn Initializer>> {
    vec![
        Box::new(initializers::startup_reindex::StartupReindex),
        Box::new(initializers::db_load_guard::LoadGuard),
    ]
}

/// Builds the base pools and shared-store seams.
///
/// An edition that brings its own rule for a seam inserts its own `Arc<dyn Trait>` afterwards: the store is keyed by `TypeId`, so the later insert wins without changing any call site.
pub(crate) async fn after_context(ctx: AppContext) -> Result<AppContext> {
    let ctx = install_services(ctx).await?;
    ctx.shared_store
        .insert(Arc::new(LocoJobDispatcher) as Arc<dyn EmbeddingSyncDispatcher>);
    ctx.shared_store
        .insert(Arc::new(LocoJobDispatcher) as Arc<dyn ReindexDispatcher>);
    Ok(ctx)
}

/// The route tree under construction, together with the inventory that `validate` checks and swagger serves.
///
/// An edition mounts its own route groups after the base ones, so it adds paths rather than replacing any.
pub(crate) struct RouteMounts {
    app_routes: AppRoutes,
    inventory: RouteInventory,
    credential_paths: Vec<&'static str>,
}

/// The unauthenticated credential paths the per-IP rate limit applies to, published by [`RouteMounts::finish`].
#[derive(Clone)]
struct CredentialPaths(Vec<&'static str>);

impl RouteMounts {
    /// The base route groups, mounted.
    pub(crate) fn community() -> Self {
        let mut inventory = RouteInventory::default();
        inventory.add_allowlisted_exclusions();
        let app_routes = AppRoutes::with_default_routes();
        inventory.add_infrastructure(&app_routes);
        let mut mounts = Self {
            app_routes,
            inventory,
            credential_paths: Vec::new(),
        };
        mounts
            .guard_credentials("/auth/signup")
            .guard_credentials("/auth/login");

        // Registers a route group with Loco and records it, with its OpenAPI document when the
        // `openapi` feature is on, in the inventory.
        macro_rules! mount {
            ($routes:expr $(, $docs:expr)?) => {{
                mounts.mount($routes);
                $(
                    #[cfg(feature = "openapi")]
                    mounts.document($docs);
                )?
            }};
        }

        mount!(
            controllers::audit_log::routes(),
            controllers::audit_log::openapi_docs()
        );
        mount!(
            controllers::api_keys::routes(),
            controllers::api_keys::openapi_docs()
        );
        mount!(
            controllers::auth::routes(),
            controllers::auth::openapi_docs()
        );
        mount!(
            controllers::entities::routes(),
            controllers::entities::openapi_docs()
        );
        mount!(controllers::entities::migration_routes());
        mount!(
            controllers::export::routes(),
            controllers::export::openapi_docs()
        );
        mount!(
            controllers::import::routes(),
            controllers::import::openapi_docs()
        );
        mount!(
            controllers::members::routes(),
            controllers::members::openapi_docs()
        );
        mount!(
            controllers::relations::routes(),
            controllers::relations::openapi_docs()
        );
        mount!(
            controllers::schemas::routes(),
            controllers::schemas::openapi_docs()
        );
        mount!(
            controllers::schemas::template_routes(),
            controllers::schemas::template_openapi_docs()
        );
        mount!(
            controllers::search::routes(),
            controllers::search::openapi_docs()
        );
        mount!(
            controllers::setup::routes(),
            controllers::setup::openapi_docs()
        );
        mount!(
            controllers::system::routes(),
            controllers::system::openapi_docs()
        );
        mount!(
            controllers::template_library::routes(),
            controllers::template_library::openapi_docs()
        );
        mount!(
            controllers::whoami::routes(),
            controllers::whoami::openapi_docs()
        );
        mount!(
            controllers::workspaces::routes(),
            controllers::workspaces::openapi_docs()
        );
        mounts
    }

    /// Registers a route group with Loco and records its operations in the inventory.
    pub(crate) fn mount(&mut self, routes: Routes) -> &mut Self {
        let group = AppRoutes::empty().add_route(routes.clone());
        self.inventory.add_group(&group, RouteClass::Public);
        let current = std::mem::replace(&mut self.app_routes, AppRoutes::empty());
        self.app_routes = current.add_route(routes);
        self
    }

    /// Applies the per-IP credential rate limit to `path`, which must be reachable without a bearer token.
    pub(crate) fn guard_credentials(&mut self, path: &'static str) -> &mut Self {
        self.credential_paths.push(path);
        self
    }

    /// Records the OpenAPI documents of the group mounted last.
    #[cfg(feature = "openapi")]
    pub(crate) fn document(
        &mut self,
        docs: impl IntoIterator<Item = controllers::route_inventory::RouteDoc>,
    ) -> &mut Self {
        self.inventory.add_docs(docs);
        self
    }

    /// Validates the inventory, publishes it to the shared store, and returns the route tree.
    ///
    /// # Panics
    /// Panics if the inventory fails validation, so a duplicate or unlisted route stops the process at boot.
    pub(crate) fn finish(self, ctx: &AppContext) -> AppRoutes {
        ctx.shared_store.insert(self.inventory);
        ctx.shared_store
            .insert(CredentialPaths(self.credential_paths));
        ctx.shared_store
            .get::<RouteInventory>()
            .expect("route inventory was just installed")
            .validate();
        self.app_routes
    }
}

/// Mounts the MCP server under `/mcp` and the swagger docs.
///
/// Rate limiting and the maintenance guard remain here because
/// Loco's `MiddlewareLayer` trait requires `tower::Layer` implementations
/// that `from_fn_with_state` does not provide (axum 0.8).  The
/// `server.middlewares:` config block is used only for Loco's built-in
/// middleware (`request_id`, `logger`).
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
    let inventory = ctx
        .shared_store
        .get::<RouteInventory>()
        .ok_or_else(|| loco_rs::Error::Message("route inventory missing".into()))?;
    let router = controllers::swagger::mount(router, &inventory);
    let router = controllers::mcp::mount(router, ctx, move |ctx| tools.server(ctx));
    let settings = ctx
        .shared_store
        .get::<crate::data::settings::Settings>()
        .ok_or_else(|| loco_rs::Error::Message("application settings missing".into()))?;
    let credential_paths = ctx
        .shared_store
        .get::<CredentialPaths>()
        .ok_or_else(|| loco_rs::Error::Message("credential paths missing".into()))?;
    let rate_limiter = std::sync::Arc::new(
        crate::controllers::middleware::rate_limit::RateLimiter::auth(&settings)
            .guarding(&credential_paths.0),
    );
    let router = router.layer(axum::middleware::from_fn_with_state(
        rate_limiter,
        crate::controllers::middleware::rate_limit::enforce,
    ));
    Ok(router.layer(axum::middleware::from_fn_with_state(
        ctx.clone(),
        crate::controllers::middleware::maintenance::maintenance_guard,
    )))
}

/// Enables Loco's built-in middleware stack (`request_id`, `logger`, etc.).
/// Custom middleware (rate limiter, maintenance guard) stays in `after_routes`
/// because Loco's `MiddlewareLayer` trait requires `tower::Layer`
/// implementations that `from_fn_with_state` does not provide (axum 0.8).
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
    stack.replace(
        "logger",
        Box::new(crate::controllers::middleware::access_log::Middleware::new(
            &logger_config,
            &ctx.environment,
        )),
    );
    stack
}

pub(crate) fn register_tasks(tasks: &mut Tasks) {
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
    tasks.register(tasks::reindex_embeddings::ReindexEmbeddings);
    tasks.register(tasks::maintenance::Maintenance);
    tasks.register(tasks::maintenance_status::MaintenanceStatus);
    tasks.register(tasks::db_load_guard::DbLoadGuard);
}

pub(crate) async fn on_shutdown(ctx: &AppContext) {
    initializers::db_load_guard::shutdown(ctx).await;
}

/// Seeds the demo tenant and workspace from `fixtures/`.
pub(crate) async fn seed(ctx: &AppContext, base: &Path) -> Result<()> {
    let fixtures = base.join("fixtures");
    let tenants = fixtures.join("tenant_tenants.yaml");
    if tenants.exists() {
        loco_rs::db::seed::<crate::models::tenant_tenants::ActiveModel>(
            &ctx.db,
            &tenants.display().to_string(),
        )
        .await?;
    }
    let workspaces = fixtures.join("workspace_workspaces.yaml");
    if workspaces.exists() {
        loco_rs::db::seed::<crate::models::workspace_workspaces::ActiveModel>(
            &ctx.db,
            &workspaces.display().to_string(),
        )
        .await?;
    }
    Ok(())
}

/// Builds the pools and shared services every entry point, tasks included, depends on.
async fn install_services(ctx: AppContext) -> Result<AppContext> {
    if ctx.is_sqlite() {
        crate::db::require_min_sqlite_connections(ctx.config.database.max_connections)
            .map_err(loco_rs::Error::Message)?;
    }

    let settings = ctx.config.settings::<crate::data::settings::Settings>()?;

    if ctx.is_postgres() {
        crate::db::install_pools(&ctx).await?;
        // Replaced by a later `shared_store.insert` when an edition brings its own rule:
        // `Arc<dyn Trait>` is keyed by `TypeId`, so the later insert wins without changing any call site.
        ctx.shared_store
            .insert(crate::controllers::middleware::auth::default_authenticator());
    }

    // Boot fails loudly if the embedding provider is misconfigured, rather than deferring the
    // error to the first search.
    let embedding_provider = crate::services::embedding::build_embedding_provider(&settings)
        .await
        .map_err(|e| loco_rs::Error::Message(format!("failed to build embedding provider: {e}")))?;
    ctx.shared_store.insert(embedding_provider);
    ctx.shared_store
        .insert(crate::workers::queue::default_queue_policy());
    // Both resolvers are installed on every backend, unlike the authenticator above: they read
    // `ctx.db` directly, and a per-workspace assignment is not an RLS concept.
    ctx.shared_store
        .insert(crate::services::embedding::default_embedding_resolver());
    ctx.shared_store
        .insert(crate::workers::embedding_sync::default_worker_class_resolver());
    // The per-workspace search token budget is request-scoped state, so it lives in `shared_store`
    // rather than being built fresh in `after_routes` like the per-IP auth limiter.
    ctx.shared_store.insert(Arc::new(
        crate::controllers::middleware::rate_limit::RateLimiter::search(&settings),
    ));
    ctx.shared_store.insert(settings);
    Ok(ctx)
}
