mod context;
mod startup;
mod workers;

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

pub use startup::StartupReindexHandle;

use crate::controllers;
use crate::controllers::route_inventory::{Edition, RouteClass, RouteInventory};

/// Refuses a request when no active licence is held, for the routes this is applied to.
///
/// This is the enterprise-edition boundary: one binary carries both editions, and the licence decides at
/// runtime which surfaces answer.
///
/// **Per request, not per boot.** Mounting the gated routes conditionally at startup would be
/// simpler and is wrong: `LicenceState::is_active` compares `exp` against the current time on every
/// call precisely so a key that lapses while the process runs stops unlocking enterprise features without
/// a restart (see `ee::services::licence`). A route set decided once at boot cannot un-mount, which
/// would turn that property into a silent enforcement hole.
///
/// Applied through `Routes::layer`, which wraps each handler's own `MethodRouter`, so it reaches
/// exactly the routes it is attached to and cannot leak onto the community ones. That is a property
/// of the data rather than of this function, but it is the reason those routes stay reachable.
///
/// Running before the handler is also what keeps an unlicensed deployment un-probeable: every gated
/// route answers the same 404 to everyone, rather than authenticating first and thereby confirming
/// to a valid key that the endpoint exists and is merely locked.
async fn licence_gate(
    axum::extract::State(ctx): axum::extract::State<AppContext>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    let active = crate::services::edition::is_active(&ctx);

    if active {
        return next.run(request).await;
    }

    // 404 rather than 402 or 403, matching the setup wizard's answer for a capability this
    // deployment does not offer: the endpoint is genuinely not being served here. The message names
    // the reason, because the operator is the one who can fix it.
    //
    // Rendered through `ApiError` so the body matches every other error this application emits
    // rather than being formatted a second way.
    crate::controllers::error::ApiError(crate::error::YorishiroError::not_found(
        "this feature requires a licence key (set YORISHIRO_LICENSE_KEY)",
    ))
    .into_response()
}

pub struct App;
#[async_trait]
impl Hooks for App {
    fn app_name() -> &'static str {
        env!("CARGO_CRATE_NAME")
    }

    /// Loads the canonical plain-YAML configuration.
    async fn load_config(env: &Environment) -> Result<Config> {
        crate::config::load(env).await
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
        // Register sqlite-vec for the test harness path (the test binary never runs main.rs).
        // The call site in main.rs already covers all CLI subcommands.
        startup::register_sqlite_extensions();

        let result = create_app::<Self, Migrator>(mode, environment, config).await?;

        // Startup reindex detection: check if any workspace's stored vectors
        // were embedded with a model that differs from the current provider.
        // If so, enqueue a non-blocking reindex so the server stays responsive
        // while vectors are updated.
        // Skip in test environments — the background task holds a `ctx.db`
        // connection that survives the test callback and races with loco's
        // `BootResultWrapper::drop` which tries `DROP DATABASE`.
        startup::after_boot(&result, environment);

        Ok(result)
    }

    async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
        Ok(vec![])
    }

    async fn after_context(ctx: AppContext) -> Result<AppContext> {
        let ctx = context::build(ctx).await?;
        crate::ee::services::boot::compose_context(&ctx);
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
        let gate = axum::middleware::from_fn_with_state(ctx.clone(), licence_gate);
        let mut inventory = RouteInventory::default();
        inventory.add_allowlisted_exclusions();
        let mut app_routes = AppRoutes::with_default_routes();
        inventory.add_infrastructure(&app_routes);

        macro_rules! mount {
            ($route:expr, $edition:expr, $gated:expr) => {{
                let route = $route;
                let inventory_routes = AppRoutes::empty().add_route(route.clone());
                inventory.add_group(&inventory_routes, $edition, $gated, RouteClass::Public);
                app_routes = app_routes.add_route(route);
            }};
        }

        mount!(controllers::audit_log::routes(), Edition::Community, false);
        inventory.add_docs(controllers::audit_log::openapi_docs());
        mount!(controllers::auth::routes(), Edition::Community, false);
        inventory.add_docs(controllers::auth::openapi_docs());
        mount!(controllers::entities::routes(), Edition::Community, false);
        inventory.add_docs(controllers::entities::openapi_docs());
        mount!(
            controllers::entities::migration_routes(),
            Edition::Community,
            false
        );
        mount!(controllers::export::routes(), Edition::Community, false);
        inventory.add_docs(controllers::export::openapi_docs());
        mount!(controllers::import::routes(), Edition::Community, false);
        inventory.add_docs(controllers::import::openapi_docs());
        mount!(controllers::members::routes(), Edition::Community, false);
        inventory.add_docs(controllers::members::openapi_docs());
        mount!(controllers::relations::routes(), Edition::Community, false);
        inventory.add_docs(controllers::relations::openapi_docs());
        mount!(controllers::schemas::routes(), Edition::Community, false);
        inventory.add_docs(controllers::schemas::openapi_docs());
        mount!(
            controllers::schemas::template_routes(),
            Edition::Community,
            false
        );
        inventory.add_docs(controllers::schemas::template_openapi_docs());
        mount!(controllers::search::routes(), Edition::Community, false);
        inventory.add_docs(controllers::search::openapi_docs());
        mount!(controllers::setup::routes(), Edition::Community, false);
        inventory.add_docs(controllers::setup::openapi_docs());
        mount!(controllers::system::routes(), Edition::Community, false);
        inventory.add_docs(controllers::system::openapi_docs());
        mount!(
            controllers::template_library::routes(),
            Edition::Community,
            false
        );
        inventory.add_docs(controllers::template_library::openapi_docs());
        mount!(controllers::whoami::routes(), Edition::Community, false);
        inventory.add_docs(controllers::whoami::openapi_docs());
        mount!(controllers::workspaces::routes(), Edition::Community, false);
        inventory.add_docs(controllers::workspaces::openapi_docs());
        // The enterprise edition's routes are mounted unconditionally; the inventory records the
        // edition boundary and the licence gate separately from runtime reachability.
        mount!(
            crate::ee::controllers::dashboard::routes(),
            Edition::Enterprise,
            false
        );
        inventory.add_docs(crate::ee::controllers::dashboard::openapi_docs());
        mount!(
            crate::ee::controllers::embedding::routes(),
            Edition::Enterprise,
            false
        );
        inventory.add_docs(crate::ee::controllers::embedding::openapi_docs());
        mount!(
            crate::ee::controllers::entity_columns::routes(),
            Edition::Enterprise,
            false
        );
        inventory.add_docs(crate::ee::controllers::entity_columns::openapi_docs());
        mount!(
            crate::ee::controllers::inference::routes(),
            Edition::Enterprise,
            false
        );
        inventory.add_docs(crate::ee::controllers::inference::openapi_docs());
        mount!(
            crate::ee::controllers::inference::gated_routes().layer(gate.clone()),
            Edition::Enterprise,
            true
        );
        inventory.add_docs(crate::ee::controllers::inference::gated_openapi_docs());
        mount!(
            crate::ee::controllers::inference::inference_job_status_routes().layer(gate.clone()),
            Edition::Enterprise,
            true
        );
        inventory.add_docs(crate::ee::controllers::inference::job_status_openapi_docs());
        mount!(
            crate::ee::controllers::marketplace::routes().layer(gate.clone()),
            Edition::Enterprise,
            true
        );
        inventory.add_docs(crate::ee::controllers::marketplace::openapi_docs());
        mount!(
            crate::ee::controllers::oauth::routes().layer(gate.clone()),
            Edition::Enterprise,
            true
        );
        inventory.add_docs(crate::ee::controllers::oauth::openapi_docs());
        mount!(
            crate::ee::controllers::origin::routes(),
            Edition::Enterprise,
            false
        );
        inventory.add_docs(crate::ee::controllers::origin::openapi_docs());
        mount!(
            crate::ee::controllers::schema_forks::routes(),
            Edition::Enterprise,
            false
        );
        inventory.add_docs(crate::ee::controllers::schema_forks::openapi_docs());
        mount!(
            crate::ee::controllers::stripe::routes().layer(gate),
            Edition::Enterprise,
            true
        );
        inventory.add_docs(crate::ee::controllers::stripe::openapi_docs());
        mount!(
            crate::ee::controllers::worker_class::routes(),
            Edition::Enterprise,
            false
        );
        inventory.add_docs(crate::ee::controllers::worker_class::openapi_docs());

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
            let enterprise_tool_router = crate::ee::services::mcp::tool_router();
            let enterprise_tool_names = enterprise_tool_router
                .map
                .keys()
                .map(ToString::to_string)
                .collect::<std::collections::HashSet<_>>();
            let mut tool_routers = vec![crate::services::mcp::community_tool_router()];
            if crate::services::edition::is_active(&ctx) {
                tool_routers.push(enterprise_tool_router);
            }
            let tool_router = crate::services::mcp::compose_tool_routers(tool_routers);
            crate::services::mcp::YorishiroMcpServer::new(ctx, tool_router, enterprise_tool_names)
        });
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
            Box::new(crate::services::access_log::Middleware::new(
                &logger_config,
                &ctx.environment,
            )),
        );
        stack
    }

    async fn connect_workers(ctx: &AppContext, queue: &Queue) -> Result<()> {
        workers::connect_workers(ctx, queue).await?;
        crate::ee::services::boot::connect_workers(ctx, queue).await?;
        Ok(())
    }

    fn register_tasks(tasks: &mut Tasks) {
        workers::register_tasks(tasks);
        crate::ee::services::boot::register_tasks(tasks);
    }
    async fn truncate(_ctx: &AppContext) -> Result<()> {
        Ok(())
    }
    /// Publishing the templates themselves stays `seed_official_templates`'s own job; this only
    /// ensures the tenant that owns them exists.
    ///
    /// That tenant is `INFRASTRUCTURE_TENANT_ID` (the nil UUID), which `models::tenancy` already
    /// knows about and already excludes from every count it takes against `YORISHIRO_MAX_TENANTS`,
    /// so seeding it cannot consume a single-tenant deployment's one slot.
    async fn seed(ctx: &AppContext, base: &Path) -> Result<()> {
        crate::ee::services::boot::seed(ctx).await?;
        // Seed from YAML fixtures (Loco db::seed).
        // Locates fixture files under the `src/fixtures/` directory and feeds
        // each to `loco_rs::db::seed::<T>()` which expects a file path string.
        let fixtures = base.join("fixtures");
        if fixtures.join("tenant_tenants.yaml").exists() {
            loco_rs::db::seed::<crate::models::tenant_tenants::ActiveModel>(
                &ctx.db,
                &fixtures.join("tenant_tenants.yaml").display().to_string(),
            )
            .await?;
        }
        if fixtures.join("workspaces.yaml").exists() {
            loco_rs::db::seed::<crate::models::workspace_workspaces::ActiveModel>(
                &ctx.db,
                &fixtures.join("workspaces.yaml").display().to_string(),
            )
            .await?;
        }
        Ok(())
    }
}
