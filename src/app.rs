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

#[allow(unused_imports)]
use crate::{controllers, tasks};

/// Refuses a request when no active licence is held, for the routes this is applied to.
///
/// This is the paid-edition boundary: one binary carries both editions, and the licence decides at
/// runtime which surfaces answer.
///
/// **Per request, not per boot.** Mounting the gated routes conditionally at startup would be
/// simpler and is wrong: `LicenceState::is_active` compares `exp` against the current time on every
/// call precisely so a key that lapses while the process runs stops unlocking paid features without
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

    let active = ctx
        .shared_store
        .get::<crate::ee::services::licence::LicenceState>()
        .is_some_and(|state| state.is_active());

    if active {
        return next.run(request).await;
    }

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

    /// Builds the RLS-aware raw sqlx pool and stores it in `shared_store`, on PostgreSQL only.
    /// `crate::db`'s module doc has why that pool exists separately from `ctx.db`.
    ///
    /// On SQLite none of this runs, and the branch must skip it rather than let it fail: `PgPoolOptions::connect` on a `sqlite://` URL hangs indefinitely instead of erroring.
    /// That backend has no second tenant to isolate (see `docs/sqlite.md`), so `DbHandle` and the `Authenticator` seam are not built at all; `controllers::extractors` authenticates against `ctx.db` directly there.
    async fn after_context(ctx: AppContext) -> Result<AppContext> {
        if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
            crate::db::require_min_sqlite_connections(ctx.config.database.max_connections)
                .map_err(loco_rs::Error::Message)?;
        }
        if ctx.db.get_database_backend() != sea_orm::DatabaseBackend::Sqlite {
            let database_url = ctx.config.database.uri.clone();
            let tenant =
                crate::db::TenantDb::connect(&database_url, ctx.config.database.max_connections)
                    .await
                    .map_err(|e| {
                        loco_rs::Error::Message(format!("failed to build tenant pool: {e}"))
                    })?;
            // The identity pool connects as the migration role for control-plane access (signup,
            // setup, the admin CLI), so it needs no hooks: it never scopes to a workspace.
            let identity = sqlx::postgres::PgPoolOptions::new()
                .max_connections(ctx.config.database.max_connections)
                .connect(&database_url)
                .await
                .map_err(|e| {
                    loco_rs::Error::Message(format!("failed to build identity pool: {e}"))
                })?;
            ctx.shared_store
                .insert(crate::db::DbHandle { tenant, identity });
            // Each of the four seams below works the same way: a later `shared_store.insert` of the
            // same trait object replaces the default without any call site changing.
            ctx.shared_store
                .insert(crate::services::auth::default_authenticator());
        }
        // Boot fails loudly if the embedding provider is misconfigured, rather than deferring the error to the first search.
        let embedding_provider = crate::services::embedding::build_embedding_provider()
            .await
            .map_err(|e| {
                loco_rs::Error::Message(format!("failed to build embedding provider: {e}"))
            })?;
        ctx.shared_store.insert(embedding_provider);
        // Both resolver seams are installed on every backend, unlike the authenticator above: they
        // read `ctx.db` directly, and a per-workspace assignment is not an RLS concept.
        ctx.shared_store
            .insert(crate::services::embedding::default_embedding_resolver());
        ctx.shared_store
            .insert(crate::workers::embedding_sync::default_worker_class_resolver());
        // Per-workspace search token budget: a request scope, so it belongs in shared_store rather than being built fresh in after_routes like the (per-IP, request-scoped-only) auth rate limiter is.
        ctx.shared_store.insert(std::sync::Arc::new(
            crate::services::rate_limit::RateLimiter::search_tokens_from_env(),
        ));

        // The paid edition's own wiring.
        //
        // An absent or invalid licence key warns and continues rather than failing boot: unlike
        // `require_min_sqlite_connections` above, this misconfiguration announces itself the moment
        // a gated route is reached, so the operator keeps the choice.
        ctx.shared_store
            .insert(crate::ee::services::licence::LicenceState::from_env());

        if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
            // The three features named here are `ee::models::origin::list_with_upstream_changes`,
            // `ee::models::marketplace::list_marketplace` and `insert_next_version`, each of which
            // hardcodes `DatabaseBackend::Postgres` and would otherwise fail at execution time
            // naming a query rather than the configuration behind it. Nothing else reports the paid
            // edition's state at boot here, since `TenantScopedAuthenticator` below is not installed.
            tracing::warn!(
                "some paid features are unavailable on SQLite: browsing the marketplace, publishing \
                 a template version, and listing template-origin updates each run a PostgreSQL-only \
                 query and will fail when reached. Point DATABASE_URL at PostgreSQL to use them; \
                 everything else works on this backend"
            );
        } else {
            // Replaces the default authenticator inserted above (`Arc<dyn Authenticator>` is keyed
            // by `TypeId`, so the later insert wins), letting a tenant-scoped key name its workspace
            // per request on every authenticated path, REST and MCP alike.
            //
            // PostgreSQL only, because it reads through `DbHandle::tenant`. Deliberately not
            // licence-conditional either: tying authentication to the licence would change who a
            // caller *is* the moment a key lapsed, rather than which features they reach.
            // Installing it unconditionally here is safe because it is a strict superset, calling
            // `authenticate_api_key($1, $2)` where base calls `($1)`, and the second argument only
            // matters for a NULL-`workspace_id` key, which nothing outside `ee/` can mint.
            ctx.shared_store.insert(std::sync::Arc::new(
                crate::ee::services::tenant_auth::TenantScopedAuthenticator,
            )
                as std::sync::Arc<dyn crate::services::auth::Authenticator>);
        }

        // The embedding and worker-class resolver seams, replacing the defaults inserted above.
        // Both return `None` for a workspace with no row of its own, so a deployment that never
        // assigns one is unaffected; which compute a tenant's jobs run on, and against which
        // embedding backend, is the paid-edition decision that keeps these tables in `ee/`.
        ctx.shared_store.insert(std::sync::Arc::new(
            crate::ee::services::embedding_resolver::EmbeddingKeyResolver,
        )
            as std::sync::Arc<dyn crate::services::embedding::WorkspaceEmbeddingResolver>);
        ctx.shared_store.insert(std::sync::Arc::new(
            crate::ee::services::worker_class_resolver::WorkerClassAssignmentResolver,
        )
            as std::sync::Arc<dyn crate::workers::embedding_sync::WorkerClassResolver>);

        Ok(ctx)
    }

    /// The community routes first, then the paid edition's on top, which is the shape the product
    /// describes: `ee/` adds paths rather than replacing any.
    ///
    /// Only `marketplace` and `inference::gated_routes` take the gate. `oauth` and `stripe` are
    /// gated by configuration instead, and the rest serve without a licence; `ee-composition.md`
    /// records why each falls where it does, and widening that set is a product decision rather than
    /// something to inherit from where a layer is attached.
    ///
    /// `inference` is two route groups because the licence line runs through the middle of it:
    /// `infer-fill` spends an LLM call and is gated, while the `/workspace/llm-key` routes beside it
    /// only store the credential. A layer applies to a whole `Routes`, so one group would gate all
    /// four.
    fn routes(_ctx: &AppContext) -> AppRoutes {
        let gate = axum::middleware::from_fn_with_state(_ctx.clone(), licence_gate);

        AppRoutes::with_default_routes()
            .add_route(controllers::audit_log::routes())
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
            // The paid edition's routes, mounted unconditionally; see this function's doc comment
            // for why only two of them carry the gate.
            .add_route(crate::ee::controllers::dashboard::routes())
            .add_route(crate::ee::controllers::embedding::routes())
            .add_route(crate::ee::controllers::entity_columns::routes())
            .add_route(crate::ee::controllers::inference::routes())
            .add_route(crate::ee::controllers::inference::gated_routes().layer(gate.clone()))
            .add_route(crate::ee::controllers::marketplace::routes().layer(gate))
            .add_route(crate::ee::controllers::oauth::routes())
            .add_route(crate::ee::controllers::origin::routes())
            .add_route(crate::ee::controllers::stripe::routes())
            .add_route(crate::ee::controllers::worker_class::routes())
    }

    /// Mounts the MCP server under `/mcp` and layers the maintenance guard and the auth rate limiter over everything, REST and MCP alike.
    /// `rmcp`'s `StreamableHttpService` is a plain `tower::Service`, not something `Hooks::routes()`/`AppRoutes` can carry, so it's mounted here instead: this hook runs after Loco's own routes are built, which is where Loco itself says custom Axum logic belongs.
    /// The middleware layers must come after the MCP mount, not before: `.layer` wraps everything already on the router at the point it's called.
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

    /// Registers all three `WorkerClass` worker types, not just one: `Queue::register` keys a handler by `class_name()`, and `enqueue_for_class` enqueues under whichever of the three types matches the resolved `WorkerClass`, so a type left unregistered here would have jobs enqueue successfully but never dequeue (loco-rs has no "unregistered handler" error at enqueue time, only silence at dequeue time).
    async fn connect_workers(ctx: &AppContext, queue: &Queue) -> Result<()> {
        use crate::workers::embedding_sync::{
            EmbeddingSyncWorkerOfficial, EmbeddingSyncWorkerShared,
            EmbeddingSyncWorkerTenantPrivate,
        };
        queue
            .register(EmbeddingSyncWorkerTenantPrivate::build(ctx))
            .await?;
        queue
            .register(EmbeddingSyncWorkerOfficial::build(ctx))
            .await?;
        queue.register(EmbeddingSyncWorkerShared::build(ctx)).await
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
        // The paid edition's tasks. Registering only makes them available to `cargo loco task`;
        // neither runs at boot, so this changes nothing for a deployment that never invokes them.
        tasks.register(crate::ee::tasks::seed_official_templates::SeedOfficialTemplates);
        tasks.register(crate::ee::tasks::create_tenant_api_key::CreateTenantApiKey);
        // tasks-inject (do not remove)
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
    async fn seed(ctx: &AppContext, _base: &Path) -> Result<()> {
        crate::ee::services::official_templates::ensure_official_tenant(&ctx.db).await?;
        Ok(())
    }
}
