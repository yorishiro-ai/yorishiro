use async_trait::async_trait;
use loco_rs::{
    Result,
    app::{AppContext, Hooks, Initializer},
    bgworker::{BackgroundWorker, Queue},
    boot::{BootResult, StartMode, create_app},
    config::Config,
    controller::AppRoutes,
    environment::Environment,
    task::Tasks,
};
use migration::Migrator;
use std::path::Path;
use std::sync::Arc;

use crate::controllers;
use crate::controllers::route_inventory::{RouteClass, RouteInventory};
use crate::db::AppContextBackend;
#[cfg(feature = "enterprise")]
use crate::ee::controllers::middleware::edition;
use crate::initializers;
use crate::tasks;
use crate::workers::dispatch::{EmbeddingSyncDispatcher, LocoJobDispatcher, ReindexDispatcher};
use crate::workers::embedding_sync::{
    EmbeddingSyncWorkerOfficial, EmbeddingSyncWorkerShared, EmbeddingSyncWorkerTenantPrivate,
};
use crate::workers::reindex::{
    ReindexWorkerOfficial, ReindexWorkerShared, ReindexWorkerTenantPrivate,
};

pub struct App;
#[async_trait]
impl Hooks for App {
    fn app_name() -> &'static str {
        env!("CARGO_CRATE_NAME")
    }

    /// Loads the canonical plain-YAML configuration.
    async fn load_config(env: &Environment) -> Result<Config> {
        crate::data::config::load(env).await
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
        crate::data::config::validate_queue_policy(&config)?;
        // Register sqlite-vec for the test harness path (the test binary never runs main.rs).
        // The call site in main.rs already covers all CLI subcommands.
        crate::db::register_sqlite_extensions();
        create_app::<Self, Migrator>(mode, environment, config).await
    }

    async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
        Ok(vec![
            Box::new(initializers::startup_reindex::StartupReindex),
            Box::new(initializers::db_load_guard::LoadGuard),
        ])
    }

    async fn after_context(ctx: AppContext) -> Result<AppContext> {
        let ctx = install_services(ctx).await?;
        ctx.shared_store
            .insert(Arc::new(LocoJobDispatcher) as Arc<dyn EmbeddingSyncDispatcher>);
        ctx.shared_store
            .insert(Arc::new(LocoJobDispatcher) as Arc<dyn ReindexDispatcher>);
        #[cfg(feature = "enterprise")]
        crate::ee::app::compose_context(&ctx);
        Ok(ctx)
    }

    /// The community routes first, then the enterprise edition's on top, which is the shape the product
    /// describes: `ee/` adds paths rather than replacing any.
    ///
    /// `marketplace`, `stripe`, `oauth` and `inference::gated_routes` take the gate, and the rest
    /// serve without a licence; `ee-composition.md` records why each falls where it does, and
    /// widening that set is a product decision rather than something to inherit from where a layer
    /// is attached.
    ///
    /// `stripe` and `oauth` are gated because billing and SSO login are enterprise-edition features, which
    /// is a decision about what each feature is rather than about what protects it. Both still need
    /// their own configuration to do anything, and the webhook still verifies its Stripe signature;
    /// neither of those made them community routes.
    ///
    /// `inference` is two route groups because the licence line runs through the middle of it:
    /// `infer-fill` spends an LLM call and is gated, while the `/workspace/llm-key` routes beside it
    /// only store the credential. A layer applies to a whole `Routes`, so one group would gate all
    /// four.
    fn routes(ctx: &AppContext) -> AppRoutes {
        #[cfg(feature = "enterprise")]
        let gate = axum::middleware::from_fn_with_state(ctx.clone(), edition::licence_gate);
        let mut inventory = RouteInventory::default();
        inventory.add_allowlisted_exclusions();
        let mut app_routes = AppRoutes::with_default_routes();
        inventory.add_infrastructure(&app_routes);

        // Registers a route group with Loco and records it, with its OpenAPI document when the
        // `openapi` feature is on, in the inventory that `validate` checks and swagger serves.
        macro_rules! mount {
            ($routes:expr $(, $docs:expr)?) => {{
                let routes = $routes;
                let group = AppRoutes::empty().add_route(routes.clone());
                inventory.add_group(&group, RouteClass::Public);
                $(
                    #[cfg(feature = "openapi")]
                    inventory.add_docs($docs);
                )?
                app_routes = app_routes.add_route(routes);
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

        // The enterprise edition's routes are mounted unconditionally; the inventory records the
        // edition boundary and the licence gate separately from runtime reachability.
        #[cfg(feature = "enterprise")]
        {
            mount!(
                crate::ee::controllers::dashboard::routes(),
                crate::ee::controllers::dashboard::openapi_docs()
            );
            mount!(
                crate::ee::controllers::embedding::routes(),
                crate::ee::controllers::embedding::openapi_docs()
            );
            mount!(
                crate::ee::controllers::entity_columns::routes(),
                crate::ee::controllers::entity_columns::openapi_docs()
            );
            mount!(
                crate::ee::controllers::inference::routes(),
                crate::ee::controllers::inference::openapi_docs()
            );
            mount!(
                crate::ee::controllers::inference::gated_routes().layer(gate.clone()),
                edition::licence_required(crate::ee::controllers::inference::gated_openapi_docs())
            );
            mount!(
                crate::ee::controllers::inference::inference_job_status_routes()
                    .layer(gate.clone()),
                edition::licence_required(
                    crate::ee::controllers::inference::job_status_openapi_docs()
                )
            );
            mount!(
                crate::ee::controllers::marketplace::routes().layer(gate.clone()),
                edition::licence_required(crate::ee::controllers::marketplace::openapi_docs())
            );
            mount!(
                crate::ee::controllers::oauth::routes().layer(gate.clone()),
                edition::licence_required(crate::ee::controllers::oauth::openapi_docs())
            );
            mount!(
                crate::ee::controllers::origin::routes(),
                crate::ee::controllers::origin::openapi_docs()
            );
            mount!(
                crate::ee::controllers::schema_forks::routes(),
                crate::ee::controllers::schema_forks::openapi_docs()
            );
            mount!(
                crate::ee::controllers::stripe::routes().layer(gate.clone()),
                edition::licence_required(crate::ee::controllers::stripe::openapi_docs())
            );
            mount!(
                crate::ee::controllers::worker_class::routes(),
                crate::ee::controllers::worker_class::openapi_docs()
            );
        }

        ctx.shared_store.insert(inventory);
        ctx.shared_store
            .get::<RouteInventory>()
            .expect("route inventory was just installed")
            .validate();
        app_routes
    }

    /// Mounts the MCP server under `/mcp` and the swagger docs.
    ///
    /// Rate limiting and the maintenance guard remain in `after_routes` because
    /// Loco's `MiddlewareLayer` trait requires `tower::Layer` implementations
    /// that `from_fn_with_state` does not provide (axum 0.8).  The
    /// `server.middlewares:` config block is used only for Loco's built-in
    /// middleware (`request_id`, `logger`).
    ///
    /// `rmcp`'s `StreamableHttpService` is a plain `tower::Service`, not
    /// something `Hooks::routes()`/`AppRoutes` can carry, so it's mounted here
    /// instead: this hook runs after Loco's own routes are built, which is where
    /// Loco itself says custom Axum logic belongs.
    async fn after_routes(router: axum::Router, ctx: &AppContext) -> Result<axum::Router> {
        let inventory = ctx
            .shared_store
            .get::<RouteInventory>()
            .ok_or_else(|| loco_rs::Error::Message("route inventory missing".into()))?;
        let router = controllers::swagger::mount(router, &inventory);
        let router = controllers::mcp::mount(router, ctx, |ctx| {
            #[cfg(feature = "enterprise")]
            let enterprise_tool_router = crate::ee::controllers::mcp::tool_router();
            #[cfg(feature = "enterprise")]
            let enterprise_tool_names = enterprise_tool_router
                .map
                .keys()
                .map(ToString::to_string)
                .collect::<std::collections::HashSet<_>>();
            #[cfg(feature = "enterprise")]
            let mut tool_routers = vec![crate::controllers::mcp::community_tool_router()];
            #[cfg(not(feature = "enterprise"))]
            let tool_routers = vec![crate::controllers::mcp::community_tool_router()];
            #[cfg(feature = "enterprise")]
            tool_routers.push(enterprise_tool_router);
            let tool_router = crate::controllers::mcp::compose_tool_routers(tool_routers);
            let server = crate::controllers::mcp::YorishiroMcpServer::new(ctx, tool_router);
            #[cfg(feature = "enterprise")]
            let server = server.with_tool_filter(move |ctx, name| {
                !enterprise_tool_names.contains(name)
                    || crate::ee::controllers::middleware::edition::is_active(ctx)
            });
            server
        });
        let settings = ctx
            .shared_store
            .get::<crate::data::settings::Settings>()
            .ok_or_else(|| loco_rs::Error::Message("application settings missing".into()))?;
        let rate_limiter = std::sync::Arc::new(
            crate::controllers::middleware::rate_limit::RateLimiter::auth(&settings),
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
    fn middlewares(
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

    async fn connect_workers(ctx: &AppContext, queue: &Queue) -> Result<()> {
        queue
            .register(EmbeddingSyncWorkerTenantPrivate::build(ctx))
            .await?;
        queue
            .register(EmbeddingSyncWorkerOfficial::build(ctx))
            .await?;
        queue
            .register(EmbeddingSyncWorkerShared::build(ctx))
            .await?;
        queue
            .register(ReindexWorkerTenantPrivate::build(ctx))
            .await?;
        queue.register(ReindexWorkerOfficial::build(ctx)).await?;
        queue.register(ReindexWorkerShared::build(ctx)).await?;
        #[cfg(feature = "enterprise")]
        crate::ee::app::connect_workers(ctx, queue).await?;
        Ok(())
    }

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
        tasks.register(tasks::reindex_embeddings::ReindexEmbeddings);
        tasks.register(tasks::maintenance::Maintenance);
        tasks.register(tasks::maintenance_status::MaintenanceStatus);
        tasks.register(tasks::db_load_guard::DbLoadGuard);
        #[cfg(feature = "enterprise")]
        crate::ee::app::register_tasks(tasks);
    }

    async fn on_shutdown(ctx: &AppContext) {
        initializers::db_load_guard::shutdown(ctx).await;
    }

    async fn truncate(_ctx: &AppContext) -> Result<()> {
        Ok(())
    }

    /// Seeds the demo tenant and workspace from `fixtures/`, after the enterprise edition has
    /// seeded what it owns.
    async fn seed(ctx: &AppContext, base: &Path) -> Result<()> {
        #[cfg(feature = "enterprise")]
        crate::ee::app::seed(ctx).await?;
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
