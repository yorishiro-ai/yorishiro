use crate::migration::Migrator;
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
use std::path::Path;
use tokio::task::spawn;

/// A handle for the startup reindex background task, stored in `shared_store` so that
/// test teardown can signal shutdown and await the task before closing pools.
///
/// Without this, `close_app_pools` would close pools while the spawned task still held
/// a connection from `ctx.db`, leaving a session on the throwaway test database and
/// causing `DROP DATABASE` to panic with "being accessed by other users".
#[derive(Clone)]
pub struct StartupReindexHandle {
    shut: std::sync::Arc<std::sync::atomic::AtomicBool>,
    task: std::sync::Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl StartupReindexHandle {
    /// Signal shutdown and await the task's completion.
    ///
    /// This structurally closes the race: if the task is mid-await when signaled,
    /// we wait for that await to return (at which point it sees the flag and exits)
    /// rather than closing pools while the task still holds a ctx.db connection.
    pub async fn shutdown_and_wait(self) {
        self.shut.store(true, std::sync::atomic::Ordering::SeqCst);
        let task = {
            let mut guard = self.task.lock().unwrap();
            guard.take()
        };
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

#[allow(unused_imports)]
use crate::{controllers, tasks};

use crate::workers::embedding_sync::WorkerClass;
use crate::workers::reindex::{
    ReindexWorkerOfficial, ReindexWorkerShared, ReindexWorkerTenantPrivate,
};

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

    let active = ctx
        .shared_store
        .get::<crate::ee::services::licence::LicenceState>()
        .is_some_and(|state| state.is_active());

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

    /// Overrides Loco's default `load_config` so that `Environment::Test` auto-selects
    /// the matching config file: `DATABASE_URL` starting with `sqlite://` loads
    /// `test_sqlite.yaml`, everything else loads `test_postgres.yaml`.
    /// Both harnesses (`request_with_create_db` and `request_with_create_sqlite`)
    /// override `config.database.uri` after loading, so the config's own URI is
    /// effectively a no-op, but the queue and server settings differ between backends.
    async fn load_config(env: &Environment) -> Result<Config> {
        let env = match env {
            Environment::Test => {
                let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
                    "postgres://loco:loco@localhost:5432/yorishiro_test".into()
                });
                let backend = if url.starts_with("sqlite://") {
                    "test_sqlite"
                } else {
                    "test_postgres"
                };
                Environment::Any(backend.into())
            }
            other => other.clone(),
        };
        env.load()
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
        crate::db::register_sqlite_extensions();

        let result = create_app::<Self, Migrator>(mode, environment, config).await?;

        // Startup reindex detection: check if any workspace's stored vectors
        // were embedded with a model that differs from the current provider.
        // If so, enqueue a non-blocking reindex so the server stays responsive
        // while vectors are updated.
        spawn_startup_reindex(result.app_context.clone());

        Ok(result)
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

        // The enterprise edition's own wiring.
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
            // naming a query rather than the configuration behind it. Nothing else reports the
            // enterprise edition's state at boot here, since `TenantScopedAuthenticator` below is
            // not installed.
            tracing::warn!(
                "some enterprise features are unavailable on SQLite: browsing the marketplace, publishing \
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
        // embedding backend, is the enterprise-edition decision that keeps these tables in `ee/`.
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
            // The enterprise edition's routes, mounted unconditionally; see this function's doc comment
            // for which of them carry the gate and why.
            .add_route(crate::ee::controllers::dashboard::routes())
            .add_route(crate::ee::controllers::embedding::routes())
            .add_route(crate::ee::controllers::entity_columns::routes())
            .add_route(crate::ee::controllers::inference::routes())
            .add_route(crate::ee::controllers::inference::gated_routes().layer(gate.clone()))
            .add_route(crate::ee::controllers::marketplace::routes().layer(gate.clone()))
            .add_route(crate::ee::controllers::oauth::routes().layer(gate.clone()))
            .add_route(crate::ee::controllers::origin::routes())
            .add_route(crate::ee::controllers::stripe::routes().layer(gate))
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

    /// Registers all three `WorkerClass` worker types and the reindex worker:
    /// `Queue::register` keys a handler by `class_name()`, and `enqueue_for_class`
    /// enqueues under whichever of the three types matches the resolved `WorkerClass`,
    /// so a type left unregistered here would have jobs enqueue successfully but never
    /// dequeue (loco-rs has no "unregistered handler" error at enqueue time, only silence
    /// at dequeue time).
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
        queue
            .register(EmbeddingSyncWorkerShared::build(ctx))
            .await?;
        queue
            .register(ReindexWorkerTenantPrivate::build(ctx))
            .await?;
        queue.register(ReindexWorkerOfficial::build(ctx)).await?;
        queue.register(ReindexWorkerShared::build(ctx)).await?;
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
        tasks.register(tasks::reindex_embeddings::ReindexEmbeddings);
        tasks.register(tasks::maintenance::Maintenance);
        tasks.register(tasks::maintenance_status::MaintenanceStatus);
        // The enterprise edition's tasks. Registering only makes them available to `cargo loco task`;
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

/// Spawns a background task that detects and enqueues startup reindex for any workspace
/// whose stored vectors were embedded with a model that differs from the current provider.
///
/// This runs after migrations have applied (see `boot` above), and in a spawned task so the
/// server stays responsive while the check runs.  On SQLite there is no embedding column,
/// so this is a no-op.
///
/// **Community edition only.**  This feature compares every workspace's stamped model name
/// against the deployment-wide provider; under EE a workspace can carry its own assignment
/// (see `ee::services::embedding_resolver`), so the comparison would flag every workspace as
/// a mismatch and reindex them with the wrong provider.  Skip when a licence is active —
/// `is_active()` evaluates `exp` against the current clock each time, so a lapsed key still
/// allows CE behaviour without a restart.
fn spawn_startup_reindex(ctx: AppContext) {
    let shut = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Clone before move into the spawned task.
    let shut_for_task = shut.clone();

    let task = std::sync::Arc::new(std::sync::Mutex::new(None::<tokio::task::JoinHandle<()>>));
    let task_for_clone = task.clone();

    let handle = StartupReindexHandle {
        shut,
        task: task_for_clone,
    };
    ctx.shared_store.insert(handle);

    let join_handle = spawn(async move {
        // Check for shutdown between each await point.
        // The pattern: do work → sleep briefly (checking shut each iteration) → do more work.
        // This ensures the task exits promptly even when blocked on an async op,
        // because the sleep loop always checks the flag.
        let mut do_work = true;
        while do_work {
            do_work = false;

            // CE-only: under EE per-workspace provider assignment makes this comparison invalid.
            if ctx
                .shared_store
                .get::<crate::ee::services::licence::LicenceState>()
                .is_some_and(|state| state.is_active())
            {
                tracing::debug!("startup reindex: enterprise licence active, skipping");
                return;
            }

            // Resolve the deployment's current provider to compare against workspace stamps.
            let provider = match crate::services::embedding::build_embedding_provider().await {
                Ok(p) => p,
                Err(err) => {
                    tracing::warn!(
                        "startup reindex: failed to build embedding provider, skipping detection: {err}"
                    );
                    return;
                }
            };
            if provider.embed_batch(&[]).await.is_err() {
                tracing::warn!("startup reindex: embedding provider must be configured");
                return;
            }

            // Fetch all workspaces that have an embedding model stamp.
            // We compare each workspace's stamped model against the provider's model name.
            // If they differ, enqueue a reindex.
            use sea_orm::{EntityTrait, QuerySelect};

            let workspaces: Vec<_> = match crate::models::identity_workspaces::Entity::find()
                .select_only()
                .column(crate::models::_entities::identity_workspaces::Column::Id)
                .column(crate::models::_entities::identity_workspaces::Column::EmbeddingModel)
                .column(crate::models::_entities::identity_workspaces::Column::EmbeddingDimensions)
                .column_as(
                    crate::models::_entities::identity_tenants::Column::EmbeddingModel,
                    "tenant_model",
                )
                .column_as(
                    crate::models::_entities::identity_tenants::Column::EmbeddingDimensions,
                    "tenant_dimensions",
                )
                .left_join(crate::models::identity_tenants::Entity)
                .into_model::<crate::services::embedding::sync::StartupReindexRow>()
                .all(&ctx.db)
                .await
            {
                Ok(ws) => ws,
                Err(err) => {
                    tracing::error!("startup reindex: failed to list workspaces: {err}");
                    return;
                }
            };

            for ws in &workspaces {
                // Check for shutdown before processing each workspace.
                if shut_for_task.load(std::sync::atomic::Ordering::SeqCst) {
                    tracing::info!("startup reindex: shutdown requested, aborting");
                    return;
                }

                let Some(stamped_model) = &ws.embedding_model else {
                    // No stamp — no reindex needed. First-write stamping will handle it.
                    continue;
                };

                if stamped_model.as_str() == provider.model_name() {
                    // Already matches — no reindex needed.
                    continue;
                }

                tracing::info!(
                    workspace_id = %ws.id,
                    stamped_model = stamped_model,
                    provider_model = provider.model_name(),
                    "startup reindex: model mismatch, enqueueing reindex"
                );

                // Resolve the worker class for this workspace and dispatch through the correct type.
                let worker_class = match crate::controllers::extractors::resolve_worker_class(
                    &ctx, ws.id,
                )
                .await
                {
                    Ok(cls) => cls,
                    Err(err) => {
                        tracing::warn!(
                            workspace_id = %ws.id,
                            error = %err.0,
                            "startup reindex: failed to resolve worker class, defaulting to shared"
                        );
                        WorkerClass::Shared
                    }
                };
                let args = super::workers::reindex::ReindexArgs {
                    workspace_id: ws.id,
                    worker_class,
                };
                if let Err(err) = super::workers::reindex::enqueue_for_class(&ctx, args).await {
                    tracing::error!(
                        workspace_id = %ws.id,
                        error = %err,
                        "startup reindex: failed to enqueue reindex"
                    );
                } else {
                    tracing::info!(
                        workspace_id = %ws.id,
                        "startup reindex: enqueue success"
                    );
                }
            }
        }
    });

    // Store the JoinHandle so shutdown_and_wait() can await task completion.
    task.lock().unwrap().replace(join_handle);
}
