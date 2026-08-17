//! The community edition's binary: everything under BUSL-1.1, and nothing from `ee/`.
//!
//! The default artifact is the one `ee/` builds, which runs exactly this feature set until a
//! licence key is present. This binary exists for the deployment that cannot have proprietary
//! code on disk at all: a distribution policy, a redistribution requirement, an audit that
//! reads the package rather than the configuration. Everyone else wants the `-ee` package,
//! which is a superset and behaves identically without a licence key.
//!
//! **It is headless.** The web UI is the paid edition's SPA and is licensed with it, so there
//! is nothing here to serve at `/`: the fallback answers `404`. The REST API, the MCP server,
//! and the admin CLI are all present and behave identically.
//!
//! Keep this file free of any path into `ee/`. That is the whole point of it, and a `use` line
//! is all it would take to make the artifact unshippable for the audience it exists for.

use anyhow::Result;
use axum::http::StatusCode;
use clap::Parser;
use yorishiro_core::db::TenantDb;
use yorishiro_server::admin::{self, AdminCommand};
use yorishiro_server::{
    AppState, bind_addr_from_env, build_app, build_embedding_provider, database_url_from_env,
    shutdown_signal,
};

/// The Yorishiro server, community edition. A plain start runs the HTTP server;
/// `yorishiro-ce-server admin ...` runs one-off administrative commands. Migrations are applied
/// on startup either way. This build contains no paid features and serves no web UI.
#[derive(Parser)]
#[command(name = "yorishiro-ce-server")]
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
}

/// Serves nothing: the SPA belongs to the paid edition, so this build has no assets to fall back
/// to. A request for `/` gets a `404` rather than an empty page pretending a UI is coming.
fn no_web_ui() -> axum::routing::MethodRouter {
    axum::routing::any(|| async { StatusCode::NOT_FOUND })
}

fn main() -> Result<()> {
    // Synchronous prologue: the calls below use `std::env::set_var`, which is unsound under
    // concurrent env access. Doing them here, before the tokio runtime starts, is what makes
    // them sound.
    //
    // SAFETY: no other thread exists at this point in `main`.
    unsafe {
        yorishiro_server::config::load_and_apply_env_overrides()?;
        // A self-hosted deployment is single-tenant unless it says otherwise, which is also what
        // enables the first-run setup wizard's REST endpoints.
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
    // Not `?`: see the paid binary. An absent DATABASE_URL exits 78 so the unit stops instead of
    // retrying forever; a database that is merely not up yet still exits 1 and is retried.
    let database_url =
        database_url_from_env().unwrap_or_else(yorishiro_server::exit_with_config_code);
    let identity_pool = sqlx::PgPool::connect(&database_url).await?;
    sqlx::migrate!("../../migrations")
        .run(&identity_pool)
        .await?;

    if let Some(Command::Admin { command }) = cli.command {
        return admin::run_with_pool(&identity_pool, command).await;
    }

    let _log_guard = yorishiro_server::logging::init()?;
    tracing::info!("database connected and migrations applied");

    let bind_addr = bind_addr_from_env();
    let tenant_db = TenantDb::connect(&database_url, 20).await?;
    let embedding_provider = build_embedding_provider()?;
    let guard_pool = identity_pool.clone();
    let state = AppState::new(tenant_db, identity_pool, embedding_provider);
    let embedding_tasks = state.embedding_tasks().clone();

    let app = build_app(state, no_web_ui());

    // Off unless `YORISHIRO_DB_LOAD_THRESHOLD` is set. A spawned task rather than a router layer,
    // so `build_app` does not bring it along and the binary has to start it.
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
    tracing::info!("yorishiro-ce-server listening on {bind_addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    // Wait for the embedding sync of already-written entities before exiting, or recently
    // created entities stay permanently missing from search. A second signal during that wait
    // exits immediately: without it an operator who interrupts again sees nothing happen until
    // the 30s timeout, since the first signal's `ctrl_c()` has already resolved.
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
