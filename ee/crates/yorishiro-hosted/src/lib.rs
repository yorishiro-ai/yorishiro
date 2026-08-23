//! The paid edition's composition seam.
//!
//! `HostedApp` is a second `loco_rs::app::Hooks` implementation, distinct from
//! `yorishiro_core::app::App`, so `cli::main::<HostedApp, Migrator>()` boots the same
//! application with `ee/`'s pieces layered on. Every method delegates to `App`'s associated fn
//! for the base behaviour first; `ee/`-only wiring (the licence gate, and later the paid
//! routes/tasks/SPA) is added around that call rather than duplicating it, so a change to base's
//! boot sequence only needs updating here if this crate actually diverges from it.
//!
//! `yorishiro-core` must never depend on this crate; this crate depends on it. See CLAUDE.md's
//! "Editions and the `ee/` boundary" for the standing rule this file exists to satisfy.

pub mod controllers;
pub mod models;
pub mod services;
pub mod tasks;

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
use yorishiro_core::app::App;

use services::licence::LicenceState;
use services::tenant_auth::TenantScopedAuthenticator;

pub struct HostedApp;

#[async_trait]
impl Hooks for HostedApp {
    fn app_name() -> &'static str {
        env!("CARGO_CRATE_NAME")
    }

    fn app_version() -> String {
        App::app_version()
    }

    async fn boot(
        mode: StartMode,
        environment: &Environment,
        config: Config,
    ) -> Result<BootResult> {
        create_app::<Self, Migrator>(mode, environment, config).await
    }

    async fn initializers(ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
        App::initializers(ctx).await
    }

    /// Runs base's own `after_context` (the RLS-aware pool, the authenticator seam) first, then
    /// resolves the licence key and stores it in `shared_store` for gated paid handlers to read.
    /// An absent or invalid key does not fail boot: the free half must keep working with no
    /// licence configured, per `services::licence`'s own doc comment.
    ///
    /// Also replaces base's `default_authenticator()` with `TenantScopedAuthenticator`.
    /// `shared_store.insert` is keyed by `TypeId`, so this later insert wins over the one
    /// `App::after_context` already made: every authenticated path in the process (REST
    /// extractors and MCP handlers alike) resolves through the one authenticator the store
    /// carries, so a key is read the same way whichever door it arrives at. Leaving the default
    /// in place would silently accept only workspace-scoped keys. Installed unconditionally,
    /// not gated on licence state: a tenant-scoped key is a structural capability, not a paid
    /// feature, matching master's own `bin/yorishiro_server.rs`.
    async fn after_context(ctx: AppContext) -> Result<AppContext> {
        let ctx = App::after_context(ctx).await?;
        ctx.shared_store.insert(LicenceState::from_env());
        ctx.shared_store
            .insert(std::sync::Arc::new(TenantScopedAuthenticator)
                as std::sync::Arc<
                    dyn yorishiro_core::services::auth::Authenticator,
                >);
        Ok(ctx)
    }

    /// Adds this crate's own routes onto base's own `AppRoutes`.
    ///
    /// `/hosted/tenant/overview` carries no licence check of its own: it is authenticated the
    /// same way master's dashboard was (tenant owner/admin membership), not licence-gated, since
    /// a self-hosted tenant's own overview is not a paid feature.
    ///
    /// `/hosted/stripe/webhook` is also always mounted, matching master: the gate is
    /// `StripeConfig::from_env().webhook_secret` being absent, which the handler itself turns
    /// into a 501, the same shape as an unconfigured `oauth_config`/`stripe_config` being `None`
    /// on master. There is no licence check here either: master's own gate is "Stripe is
    /// configured but no active licence" logged as a warning at boot, not a route omission (see
    /// its `bin/yorishiro_server.rs`); this branch has no equivalent boot-time warning yet,
    /// tracked as a follow-up rather than blocking the route.
    ///
    /// `/hosted/marketplace/**` is the first route in this crate to read `LicenceState` from
    /// `shared_store`: an unlicensed deployment gets a 404 from every marketplace route, matching
    /// master's `licensed_tenant` (the marketplace is cross-tenant distribution, not part of the
    /// free floor the dashboard and Stripe webhook stay open under).
    ///
    /// `controllers::origin` overlays base's own `/api/schemas` namespace at the same paths
    /// master uses (`/upstream-changes`, `/{schema_id}/merge-preview`, `/{schema_id}/merge`):
    /// none of the three collides with base's own `/api/schemas`, `/api/schemas/{schema_id}` (a
    /// static segment always resolves ahead of a path parameter in the same position, which is
    /// what lets `/upstream-changes` coexist with `/{schema_id}`), so unlike marketplace and the
    /// dashboard this one is not under `/hosted`: merging a schema is a schema operation from the
    /// client's side, not an administrative one.
    ///
    /// `controllers::entity_columns` is the same case: master's own `/api/workspace/entity-columns`
    /// paths, kept as-is, since base's own workspace routes are `api/workspaces` (plural) and
    /// there is no collision to route around.
    ///
    /// `controllers::inference` mounts under `/hosted` (`workspace/llm-key`,
    /// `schemas/active/{name}/infer-fill`, `migration-jobs/{job_id}/{proposals,confirm}`) rather
    /// than at master's own paths, an unchecked deviation from the empirical-check method
    /// `origin` and `entity_columns` both used: nothing here confirmed a real conflict with
    /// base's own routes forced the `/hosted` prefix. Recorded alongside `marketplace`'s own
    /// unchecked `/hosted/marketplace` deviation as an API-contract question to reconcile before
    /// the PR, not fixed here mid-slice.
    ///
    /// `controllers::oauth` mounts at master's own paths (`/auth/oauth/status|authorize|callback`):
    /// no empirical check was needed here, since base's own `controllers::auth` owns `/auth`
    /// but defines nothing under `/auth/oauth`, so there is no path to collide with in the first
    /// place. Not licence-gated, the same reasoning as the Stripe webhook: an unconfigured
    /// `OAuthConfig` (no `YORISHIRO_OAUTH_ISSUER_URL`) is what the routes gate on instead.
    /// `OAuthConfig::from_env()` is read fresh on every `authorize`/`callback`/`status` request,
    /// matching `StripeConfig::from_env()`'s own per-request read, rather than cached in
    /// `shared_store` at boot: this crate has no DI seam for either, so a test configures either
    /// the same way production does, by setting the process environment for the request's
    /// duration.
    fn routes(ctx: &AppContext) -> AppRoutes {
        App::routes(ctx)
            .add_route(controllers::dashboard::routes())
            .add_route(controllers::entity_columns::routes())
            .add_route(controllers::inference::routes())
            .add_route(controllers::marketplace::routes())
            .add_route(controllers::oauth::routes())
            .add_route(controllers::origin::routes())
            .add_route(controllers::stripe::routes())
    }

    /// Delegates unchanged: base's `after_routes` mounts `/mcp` and layers the rate limiter and
    /// maintenance guard over everything, REST and MCP alike (see its own doc comment).
    ///
    /// Master's `HostedMcpServer` (`http::mcp` there) wraps `YorishiroMcpServer` so this crate
    /// can add its own MCP tools alongside base's, the same shape `routes()` uses for REST. It
    /// is deliberately not ported: `controllers::mcp::mount` (base) hardcodes
    /// `YorishiroMcpServer` as `StreamableHttpService`'s concrete type parameter (`rmcp`'s
    /// `ServerHandler` bound there is not object-safe, so this cannot be swapped via
    /// `shared_store` the way `TenantScopedAuthenticator` replaces the default authenticator),
    /// so swapping the server means re-implementing `mount` plus both middleware layers here,
    /// and a re-implementation that silently drops the rate limiter or the maintenance guard is
    /// exactly the "gate that was never made to fire" failure shape recorded elsewhere. Not
    /// worth that risk for what master's own wrapper actually does today: nothing. Its
    /// `list_tools`/`call_tool`/`get_tool` all delegate to an empty tool router, so `tools/list`
    /// answers exactly base's own tool set either way. Revisit when this crate's first MCP-only
    /// tool needs the seam, which is also when the actual composition shape becomes clear rather
    /// than guessed at for a currently-empty router.
    async fn after_routes(router: axum::Router, ctx: &AppContext) -> Result<axum::Router> {
        App::after_routes(router, ctx).await
    }

    async fn connect_workers(ctx: &AppContext, queue: &Queue) -> Result<()> {
        App::connect_workers(ctx, queue).await
    }

    /// Adds this crate's own tasks onto base's own. `Tasks::register` is purely additive, unlike
    /// `after_routes`'s MCP mount (see this method's own doc comment for why the MCP seam,
    /// `HostedMcpServer` on master, is deliberately not ported here yet).
    fn register_tasks(tasks: &mut Tasks) {
        App::register_tasks(tasks);
        tasks.register(tasks::seed_official_templates::SeedOfficialTemplates);
        tasks.register(tasks::create_tenant_api_key::CreateTenantApiKey);
    }

    async fn truncate(ctx: &AppContext) -> Result<()> {
        App::truncate(ctx).await
    }

    /// Creates the official-templates publisher tenant, so it exists on any deployment that has
    /// run `cargo loco db seed` even if `seed_official_templates` (a separate, opt-in task) has
    /// not. Publishing the templates themselves stays that task's own job: seeding is "the
    /// database's foundation is in place", not "the marketplace has content".
    async fn seed(ctx: &AppContext, base: &Path) -> Result<()> {
        App::seed(ctx, base).await?;
        services::official_templates::ensure_official_tenant(&ctx.db).await?;
        Ok(())
    }
}
