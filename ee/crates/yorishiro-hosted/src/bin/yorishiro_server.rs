use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::StreamableHttpService;
use yorishiro_core::db::TenantDb;
use yorishiro_hosted::http::controllers::stripe::StripeConfig;
use yorishiro_hosted::services::oauth::OAuthConfig;
use yorishiro_hosted::state::HostedState;
use yorishiro_server::admin::{self, AdminCommand};
use yorishiro_server::http::middleware::rate_limit::{RateLimiter, apply_rate_limit_layer};
use yorishiro_server::{
    AppState, apply_body_limit_layer, apply_observability_layers, build_app_with_rate_limiter,
    build_embedding_provider, database_url_from_env, shutdown_signal,
};

/// The Yorishiro server, enterprise edition. A plain start runs the HTTP server and serves the
/// web UI; `yorishiro-server admin ...` runs one-off administrative commands. Migrations are
/// applied on startup either way. Paid features need a licence key in `YORISHIRO_LICENSE_KEY`;
/// without one the server runs normally and those features answer 404.
#[derive(Parser)]
#[command(name = "yorishiro-server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Tenant and API key management, embedding resync.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// Issue a tenant-scoped API key: one key that can act on any workspace in the tenant,
    /// naming the workspace per request with the `X-Workspace-Id` header.
    ///
    /// Separate from `admin create-api-key`, which always binds a key to one workspace. Prefer that one when a client only ever works in a
    /// single workspace -- a key bound to one workspace reaches less if it leaks.
    CreateTenantApiKey {
        tenant_id: uuid::Uuid,
        /// `read`, `write`, or `schema`.
        scope: String,
        /// Attribute the key to a specific user. The requested scope is capped by that user's
        /// tenant role, exactly as `admin create-api-key` caps it.
        #[arg(long)]
        user: Option<uuid::Uuid>,
    },
    /// Publish the built-in templates as official marketplace listings.
    ///
    /// Idempotent: a template already published at the same definition is left alone, and one
    /// whose definition changed gets a new version rather than an edit in place. Safe to run on
    /// every deployment.
    SeedOfficialTemplates,
}

/// One directory, applied once.
///
/// `set_ignore_missing(true)` is deliberately absent. It existed because two directories shared
/// one `_sqlx_migrations` table and each pass saw the other's versions as missing; with one set
/// there is nothing legitimate for it to ignore, and keeping it would silence a genuinely absent
/// migration -- including the fresh-database boundary this version declares, which is supposed
/// to refuse loudly.
async fn run_migrations(pool: &sqlx::PgPool) -> Result<()> {
    sqlx::migrate!("../../../migrations").run(pool).await?;
    Ok(())
}

fn main() -> Result<()> {
    // Synchronous prologue: both calls below use `std::env::set_var`, which is unsound under
    // concurrent env access. Doing them here, before the tokio runtime starts, is what makes
    // them sound.
    //
    // SAFETY: no other thread exists at this point in `main`.
    unsafe {
        // Before the config file, not after. `load_and_apply_env_overrides` only sets a variable
        // that is unset, so running the aliases second would let a config-file value beat an
        // explicitly exported old name -- silently inverting the precedence a deployment already
        // depends on.
        yorishiro_server::config::aliases::apply();
        yorishiro_server::config::load_and_apply_env_overrides()?;
        // Default to a single-tenant deployment, which is what a self-hoster gets and what
        // enables the first-run setup wizard. A hosted deployment sets this to `0` (unlimited)
        // in its own environment, which disables the wizard -- it onboards through checkout or
        // an invite instead. This binary serves both, so the default belongs here rather than
        // being hardcoded: it was `0` unconditionally while a separate community binary carried
        // the self-hosted path.
        if std::env::var_os("YORISHIRO_MAX_TENANTS").is_none() {
            std::env::set_var("YORISHIRO_MAX_TENANTS", "1");
        }
    }

    let cli = Cli::parse();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli))
}

async fn run(cli: Cli) -> Result<()> {
    // Not `?`: an absent DATABASE_URL exits 78 so the unit's `RestartPreventExitStatus=78` stops
    // rather than retrying every five seconds forever. Everything below keeps exiting 1, which
    // is what `Restart=on-failure` is for -- a database still starting is worth waiting for, and
    // missing configuration is not.
    let database_url =
        database_url_from_env().unwrap_or_else(yorishiro_server::exit_with_config_code);
    let identity_pool = sqlx::PgPool::connect(&database_url).await?;
    run_migrations(&identity_pool).await?;

    match cli.command {
        Some(Command::Admin { command }) => {
            return admin::run_with_pool(&identity_pool, command).await;
        }
        Some(Command::CreateTenantApiKey {
            tenant_id,
            scope,
            user,
        }) => {
            let created = yorishiro_hosted::services::tenant_auth::create_tenant_api_key(
                &identity_pool,
                tenant_id,
                &scope,
                user,
            )
            .await?;
            println!(
                "tenant-scoped api key created (the plaintext key is shown ONLY once — store it now)"
            );
            println!("  key:          {}", created.plaintext);
            println!("  key id:       {}", created.id);
            println!("  tenant id:    {tenant_id}");
            println!("  workspace id: (send X-Workspace-Id on each request)");
            println!("  scope:        {scope}");
            return Ok(());
        }
        Some(Command::SeedOfficialTemplates) => {
            let outcome = yorishiro_hosted::services::official_templates::seed_official_templates(
                &identity_pool,
            )
            .await?;
            println!(
                "official templates: {} published, {} updated, {} unchanged",
                outcome.published.len(),
                outcome.updated.len(),
                outcome.unchanged.len()
            );
            for name in outcome.published.iter().chain(outcome.updated.iter()) {
                println!("  {name}");
            }
            return Ok(());
        }
        None => {}
    }

    let _log_guard = yorishiro_server::logging::init()?;
    tracing::info!("database connected and migrations applied");

    let bind_addr = yorishiro_hosted::services::bind_addr_from_env();

    let tenant_db = TenantDb::connect(&database_url, 20).await?;
    let embedding_provider = build_embedding_provider()?;
    // Installing this here is what makes tenant-scoped keys work at all: every authenticated
    // path in the process -- REST extractors and MCP handlers alike -- resolves through the one
    // authenticator the state carries, so a key is read the same way whichever door it arrives
    // at. Leaving it at the default would silently accept only workspace-scoped keys.
    let app_state = AppState::new(tenant_db.clone(), identity_pool.clone(), embedding_provider)
        .with_authenticator(std::sync::Arc::new(
            yorishiro_hosted::services::tenant_auth::TenantScopedAuthenticator,
        ));
    let embedding_tasks = app_state.embedding_tasks().clone();
    // Kept for the load guard below, which is spawned after `identity_pool` moves into the state.
    let guard_pool = identity_pool.clone();

    // Logs which mode this process booted into. An operator who set a key and still sees the paid
    // features closed needs that line to tell a rejected key from an absent one.
    let licence = yorishiro_hosted::services::licence::LicenceState::from_env();

    // Stripe and OAuth are gated by simply not configuring them without a licence, which reuses
    // the `None`/unconfigured paths both already have -- their routes answer 404 exactly as they
    // do on a deployment that never set the variables. Unlike the marketplace and infer-fill
    // gates, this is decided once at startup: both read process-wide configuration at boot
    // anyway, so a key expiring mid-run leaves them configured until the next restart. The two
    // request-time gates are the ones that close immediately.
    let licensed = licence.is_active();
    if !licensed {
        if StripeConfig::from_env().webhook_secret.is_some() {
            tracing::warn!("Stripe is configured but no active licence: billing routes are off");
        }
        if OAuthConfig::from_env().is_some() {
            tracing::warn!("OAuth is configured but no active licence: SSO routes are off");
        }
    }

    let hosted_state = HostedState {
        identity_pool,
        tenant_db,
        stripe_config: if licensed {
            StripeConfig::from_env()
        } else {
            StripeConfig::default()
        },
        oauth_config: if licensed {
            OAuthConfig::from_env()
        } else {
            None
        },
        licence,
    };

    let static_fallback =
        yorishiro_hosted::web::fallback_service(yorishiro_hosted::services::web_dir_from_env());

    // Shared with `build_app_with_rate_limiter` below so `/auth/oauth/authorize|callback` draw
    // from the same quota as this crate's own `/auth/login`/`/auth/signup`/`/setup*` -- an
    // attacker who exhausts one doesn't get a fresh bucket by switching to the other. See
    // `yorishiro_hosted::router`'s doc comment for why the OAuth login pair is a separate
    // sub-router from the rest of `yorishiro-hosted`'s routes.
    let rate_limiter = Arc::new(RateLimiter::from_env());
    let oauth_login_router =
        apply_body_limit_layer(apply_observability_layers(apply_rate_limit_layer(
            yorishiro_hosted::oauth_login_router().with_state(hosted_state.clone()),
            rate_limiter.clone(),
        )));
    // `/hosted/tenant/overview` (bearer-token-authenticated) and `/auth/oauth/status` (unlimited
    // by design -- the Web UI's login page polls it on every load) don't need the rate limiter,
    // but every route in this process still needs the body-size cap and observability stack
    // `build_app`'s own routes get -- `axum::Router::merge` doesn't propagate a `.layer()` from
    // either side, so each sub-router carries its own copy. `/hosted/stripe/webhook` in
    // particular must keep the body limit (an unbounded webhook body is its own DoS vector) but
    // must never be rate-limited: dropping a legitimate Stripe billing event on a `429` is worse
    // than not rate-limiting a signature-verified webhook Stripe itself, not an attacker, calls.
    // `/mcp` is served here rather than being left to `build_app`, so this edition's own tools
    // reach the same door the base edition's do. The wrapper delegates to the base server, so
    // overriding the path serves both sets rather than replacing one with the other. It must
    // define every method `build_app`'s own `/mcp` does, since overriding a path overrides
    // every method on it -- `nest_service` matches whatever the inner service accepts, as the
    // base router's does.
    let hosted_mcp = StreamableHttpService::new(
        {
            let state = app_state.clone();
            move || {
                Ok(yorishiro_hosted::http::mcp::HostedMcpServer::new(
                    state.clone(),
                ))
            }
        },
        LocalSessionManager::default().into(),
        Default::default(),
    );
    let hosted_router = apply_body_limit_layer(apply_observability_layers(
        yorishiro_hosted::router()
            .with_state(hosted_state)
            .nest_service("/mcp", hosted_mcp),
    ));
    // The maintenance guard is applied inside `build_app`, and `merge`/`fallback_service`
    // propagate a layer no more than `.layer()` does -- so without this, pausing the
    // deployment would refuse `/api/*` while this crate's own routes kept writing. Applied
    // here rather than to the merged router so it sits inside the observability stack, as it
    // does in the community edition. `/up` and `/health` opt out inside the guard itself.
    let hosted_router = hosted_router.layer(axum::middleware::from_fn_with_state(
        app_state.clone(),
        yorishiro_server::http::middleware::maintenance::maintenance_guard,
    ));
    let oauth_login_router = oauth_login_router.layer(axum::middleware::from_fn_with_state(
        app_state.clone(),
        yorishiro_server::http::middleware::maintenance::maintenance_guard,
    ));
    // This crate's routes are matched *first*, with the community edition behind them as the
    // fallback, rather than merged alongside. `Router::merge` panics on a duplicate path, so a
    // merged layout can only ever add paths the community edition does not already serve --
    // this one can also replace them, which is what lets a hosted-only behaviour take over an
    // endpoint the community edition defines.
    //
    // The community edition's own router still ends in its static-asset fallback, so an
    // unmatched path reaches the SPA exactly as before: this crate's routes, then the community
    // edition's, then `index.html`.
    //
    // One consequence to keep in mind when adding a route here: overriding a path overrides
    // **every method on it**. A request whose path matches here but whose method does not gets
    // this router's `405`, and never reaches the community edition's handler for that method.
    // Define every method a path needs, or leave the path alone.
    let base_app = build_app_with_rate_limiter(app_state, static_fallback, rate_limiter);
    let app = hosted_router
        .merge(oauth_login_router)
        .fallback_service(base_app);

    // Load shedding is started by the binary, not by `build_app` -- it is a spawned task, not a
    // router layer, so embedding the community router does not bring it along. Both editions
    // point at one database, so a guard only the community binary ran would watch a pool this
    // process is the one loading. Same env vars and same default (off unless
    // `YORISHIRO_DB_LOAD_THRESHOLD` is set), so a deployment configures both editions identically.
    if let Some(guard) = yorishiro_core::services::db_load_guard::LoadGuardConfig::from_env() {
        tracing::info!(
            threshold = guard.threshold,
            "db load guard enabled: read-only above this many active connections"
        );
        tokio::spawn(yorishiro_core::services::db_load_guard::run(
            guard_pool, guard,
        ));
    }

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("yorishiro-server listening on {bind_addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    // After closing HTTP, wait for the embedding sync of already-written entities to finish.
    // Exiting immediately would leave recently created entities permanently missing from search.
    // A second Ctrl-C/SIGTERM during this wait forces an immediate exit: without it, an operator
    // who interrupts again out of impatience sees no response at all until the 30s timeout, since
    // the first signal's `ctrl_c()` in `shutdown_signal` has already resolved and nothing else is
    // listening for a repeat.
    embedding_tasks.close();
    tokio::select! {
        result = tokio::time::timeout(std::time::Duration::from_secs(30), embedding_tasks.wait()) => {
            if result.is_err() {
                tracing::warn!(
                    "embedding syncs did not finish within 30s; exiting anyway \
                     (recover with `admin resync-embeddings`)"
                );
            }
        }
        _ = shutdown_signal() => {
            tracing::warn!(
                "second interrupt received; exiting immediately without waiting for embedding \
                 syncs to finish (recover with `admin resync-embeddings`)"
            );
        }
    }

    Ok(())
}
