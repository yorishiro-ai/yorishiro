mod context;
mod dispatch;
mod routes;
mod seed;
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
use std::sync::Arc;

use crate::controllers;
use crate::controllers::route_inventory::RouteInventory;
#[cfg(feature = "enterprise")]
use crate::controllers::route_inventory::{Edition, RouteClass};
use crate::workers::dispatch::{EmbeddingSyncDispatcher, ReindexDispatcher};
use dispatch::LocoJobDispatcher;

async fn queue_concurrency_policy(
    ctx: &AppContext,
    workspace_id: uuid::Uuid,
    class: crate::workers::embedding_sync::WorkerClass,
) -> Result<(String, i32), String> {
    #[cfg(not(feature = "enterprise"))]
    {
        let _ = (ctx, workspace_id, class);
        Ok(("community".to_owned(), 1))
    }
    #[cfg(feature = "enterprise")]
    {
        let tenant_id = crate::models::workspace_workspaces::find_tenant_id(&ctx.db, workspace_id)
            .await
            .map_err(|error| format!("queue policy lookup failed for workspace: {error}"))?;
        let Some(tenant_id) = tenant_id else {
            return Err("queue policy unavailable: workspace does not exist".into());
        };
        let licence_plan = ctx
            .shared_store
            .get::<std::sync::Arc<crate::ee::services::licence::LicenceState>>()
            .ok_or_else(|| "queue policy unavailable: licence state is missing".to_owned())?
            .active_plan_at(chrono::Utc::now().timestamp());
        let plan = if let Some(plan) = licence_plan {
            plan
        } else {
            let billing = crate::models::tenant_billing::find_plan(&ctx.db, tenant_id)
                .await
                .map_err(|error| format!("queue policy lookup failed for billing: {error}"))?;
            match billing {
                None => crate::ee::services::plan::Plan::Free,
                Some(plan) => {
                    let value = plan.ok_or_else(|| {
                        "queue policy unavailable: billing plan is missing".to_owned()
                    })?;
                    crate::ee::services::plan::Plan::from_db_str(&value)
                        .map_err(|error| error.to_string())?
                }
            }
        };
        let limit = match class {
            crate::workers::embedding_sync::WorkerClass::Official => {
                plan.compute_policy().base_official_concurrency as i32
            }
            crate::workers::embedding_sync::WorkerClass::TenantPrivate
            | crate::workers::embedding_sync::WorkerClass::Shared => 1,
        };
        Ok((plan.as_str().to_owned(), limit))
    }
}

#[async_trait]
#[cfg(feature = "enterprise")]
impl crate::ee::workers::infer_fill::InferFillDispatcher for LocoJobDispatcher {
    async fn dispatch(
        &self,
        ctx: &AppContext,
        mut args: crate::ee::workers::infer_fill::InferFillArgs,
    ) -> loco_rs::Result<String> {
        use loco_rs::bgworker::BackgroundWorker;

        let lifecycle_id = uuid::Uuid::now_v7();
        let scheduling =
            crate::services::queue::decide(crate::workers::embedding_sync::WorkerClass::Shared);
        tracing::info!(
            lifecycle_id = %lifecycle_id,
            worker_class = "shared",
            scheduling_priority = scheduling.priority,
            fallback = scheduling.fallback,
            "queue scheduling decision"
        );
        crate::models::queue_job_lifecycles::Entity::record_enqueue(
            &ctx.db,
            crate::models::queue_job_lifecycles::Enqueue {
                id: lifecycle_id,
                job_name: "infer_fill",
                worker_class: "shared",
                workspace_id: Some(args.workspace_id),
                plan: None,
                concurrency_key: Some("shared"),
                concurrency_limit: Some(1),
            },
        )
        .await
        .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
        args.lifecycle_id = Some(lifecycle_id);
        let result = crate::ee::workers::infer_fill::InferFillWorker::perform_later_with_priority(
            ctx,
            args,
            Some(scheduling.priority),
        )
        .await;
        match result {
            Ok(job_id) => {
                if let Err(error) = crate::models::queue_job_lifecycles::Entity::mark_dispatched(
                    &ctx.db,
                    lifecycle_id,
                    &job_id,
                )
                .await
                {
                    tracing::error!(
                        lifecycle_id = %lifecycle_id,
                        provider_job_id = %job_id,
                        diagnostic = %error,
                        "provider job dispatched but lifecycle correlation write failed"
                    );
                }
                Ok(job_id)
            }
            Err(error) => {
                let _ = crate::models::queue_job_lifecycles::Entity::finish(
                    &ctx.db,
                    lifecycle_id,
                    None,
                    crate::models::queue_job_lifecycles::LifecycleStatus::Unavailable,
                    Some(&error.to_string()),
                )
                .await;
                Err(error)
            }
        }
    }
}

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
#[cfg(feature = "enterprise")]
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
        crate::config::validate_queue_policy(&config)?;
        // Register sqlite-vec for the test harness path (the test binary never runs main.rs).
        // The call site in main.rs already covers all CLI subcommands.
        startup::register_sqlite_extensions();
        let serves_http = startup::serves_http(&mode);

        let result = create_app::<Self, Migrator>(mode, environment, config).await?;

        // Reindexes workspaces whose vectors came from a different model and starts the load monitor.
        startup::after_boot(&result, environment, serves_http).await?;

        Ok(result)
    }

    async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
        Ok(vec![])
    }

    async fn after_context(ctx: AppContext) -> Result<AppContext> {
        let ctx = context::build(ctx).await?;
        ctx.shared_store
            .insert(Arc::new(LocoJobDispatcher) as Arc<dyn EmbeddingSyncDispatcher>);
        ctx.shared_store
            .insert(Arc::new(LocoJobDispatcher) as Arc<dyn ReindexDispatcher>);
        #[cfg(feature = "enterprise")]
        ctx.shared_store.insert(Arc::new(LocoJobDispatcher)
            as Arc<dyn crate::ee::workers::infer_fill::InferFillDispatcher>);
        #[cfg(feature = "enterprise")]
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
        #[cfg(feature = "enterprise")]
        let gate = axum::middleware::from_fn_with_state(ctx.clone(), licence_gate);
        #[cfg(feature = "enterprise")]
        let (mut app_routes, mut inventory) = routes::community();
        #[cfg(not(feature = "enterprise"))]
        let (app_routes, inventory) = routes::community();

        #[cfg(feature = "enterprise")]
        macro_rules! mount {
            ($route:expr, $edition:expr, $gated:expr) => {{
                let route = $route;
                let inventory_routes = AppRoutes::empty().add_route(route.clone());
                inventory.add_group(&inventory_routes, $edition, $gated, RouteClass::Public);
                app_routes = app_routes.add_route(route);
            }};
        }

        #[cfg(feature = "enterprise")]
        {
            // The enterprise edition's routes are mounted unconditionally; the inventory records the
            // edition boundary and the licence gate separately from runtime reachability.
            mount!(
                crate::ee::controllers::dashboard::routes(),
                Edition::Enterprise,
                false
            );
            #[cfg(feature = "openapi")]
            inventory.add_docs(crate::ee::controllers::dashboard::openapi_docs());
            mount!(
                crate::ee::controllers::embedding::routes(),
                Edition::Enterprise,
                false
            );
            #[cfg(feature = "openapi")]
            inventory.add_docs(crate::ee::controllers::embedding::openapi_docs());
            mount!(
                crate::ee::controllers::entity_columns::routes(),
                Edition::Enterprise,
                false
            );
            #[cfg(feature = "openapi")]
            inventory.add_docs(crate::ee::controllers::entity_columns::openapi_docs());
            mount!(
                crate::ee::controllers::inference::routes(),
                Edition::Enterprise,
                false
            );
            #[cfg(feature = "openapi")]
            inventory.add_docs(crate::ee::controllers::inference::openapi_docs());
            mount!(
                crate::ee::controllers::inference::gated_routes().layer(gate.clone()),
                Edition::Enterprise,
                true
            );
            #[cfg(feature = "openapi")]
            inventory.add_docs(crate::ee::controllers::inference::gated_openapi_docs());
            mount!(
                crate::ee::controllers::inference::inference_job_status_routes()
                    .layer(gate.clone()),
                Edition::Enterprise,
                true
            );
            #[cfg(feature = "openapi")]
            inventory.add_docs(crate::ee::controllers::inference::job_status_openapi_docs());
            mount!(
                crate::ee::controllers::marketplace::routes().layer(gate.clone()),
                Edition::Enterprise,
                true
            );
            #[cfg(feature = "openapi")]
            inventory.add_docs(crate::ee::controllers::marketplace::openapi_docs());
            mount!(
                crate::ee::controllers::oauth::routes().layer(gate.clone()),
                Edition::Enterprise,
                true
            );
            #[cfg(feature = "openapi")]
            inventory.add_docs(crate::ee::controllers::oauth::openapi_docs());
            mount!(
                crate::ee::controllers::origin::routes(),
                Edition::Enterprise,
                false
            );
            #[cfg(feature = "openapi")]
            inventory.add_docs(crate::ee::controllers::origin::openapi_docs());
            mount!(
                crate::ee::controllers::schema_forks::routes(),
                Edition::Enterprise,
                false
            );
            #[cfg(feature = "openapi")]
            inventory.add_docs(crate::ee::controllers::schema_forks::openapi_docs());
            mount!(
                crate::ee::controllers::stripe::routes().layer(gate),
                Edition::Enterprise,
                true
            );
            #[cfg(feature = "openapi")]
            inventory.add_docs(crate::ee::controllers::stripe::openapi_docs());
            mount!(
                crate::ee::controllers::worker_class::routes(),
                Edition::Enterprise,
                false
            );
            #[cfg(feature = "openapi")]
            inventory.add_docs(crate::ee::controllers::worker_class::openapi_docs());
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
            let enterprise_tool_router = crate::ee::services::mcp::tool_router();
            #[cfg(feature = "enterprise")]
            let enterprise_tool_names = enterprise_tool_router
                .map
                .keys()
                .map(ToString::to_string)
                .collect::<std::collections::HashSet<_>>();
            #[cfg(feature = "enterprise")]
            let mut tool_routers = vec![crate::services::mcp::community_tool_router()];
            #[cfg(not(feature = "enterprise"))]
            let tool_routers = vec![crate::services::mcp::community_tool_router()];
            #[cfg(not(feature = "enterprise"))]
            let enterprise_tool_names = std::collections::HashSet::new();
            #[cfg(feature = "enterprise")]
            if crate::services::edition::is_active(&ctx) {
                tool_routers.push(enterprise_tool_router);
            }
            let tool_router = crate::services::mcp::compose_tool_routers(tool_routers);
            crate::services::mcp::YorishiroMcpServer::new(ctx, tool_router, enterprise_tool_names)
        });
        let settings = ctx
            .shared_store
            .get::<crate::config::Settings>()
            .ok_or_else(|| loco_rs::Error::Message("application settings missing".into()))?;
        let rate_limiter =
            std::sync::Arc::new(crate::services::rate_limit::RateLimiter::auth(&settings));
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
        #[cfg(feature = "enterprise")]
        crate::ee::services::boot::connect_workers(ctx, queue).await?;
        Ok(())
    }

    fn register_tasks(tasks: &mut Tasks) {
        workers::register_tasks(tasks);
        #[cfg(feature = "enterprise")]
        crate::ee::services::boot::register_tasks(tasks);
    }

    async fn on_shutdown(ctx: &AppContext) {
        startup::shutdown(ctx).await;
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
        #[cfg(feature = "enterprise")]
        crate::ee::services::boot::seed(ctx).await?;
        seed::community(ctx, base).await
    }
}
