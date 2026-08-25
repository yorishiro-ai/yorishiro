//! The paid edition's composition seam.
//!
//! `HostedApp` is a second `loco_rs::app::Hooks` implementation, distinct from `yorishiro_core::app::App`.
//! Every method delegates to `App`'s associated fn for the base behaviour first; `ee/`-only wiring is added around that call rather than duplicating it.
//!
//! `yorishiro-core` must never depend on this crate; this crate depends on it.

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

use services::embedding_resolver::EmbeddingKeyResolver;
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

    /// An absent or invalid licence key does not fail boot.
    async fn after_context(ctx: AppContext) -> Result<AppContext> {
        let ctx = App::after_context(ctx).await?;
        ctx.shared_store.insert(LicenceState::from_env());
        ctx.shared_store
            .insert(std::sync::Arc::new(TenantScopedAuthenticator)
                as std::sync::Arc<
                    dyn yorishiro_core::services::auth::Authenticator,
                >);
        // The embedding resolver seam: a workspace with its own row in identity_workspace_embedding_keys
        // uses that provider instead of the deployment default (see WorkspaceEmbeddingResolver's own doc
        // comment). Tenant-level assignment is the paid-edition decision, the same reasoning that keeps
        // llm_keys in ee/: base only needs to be able to *receive* a different provider per workspace.
        ctx.shared_store
            .insert(std::sync::Arc::new(EmbeddingKeyResolver)
                as std::sync::Arc<
                    dyn yorishiro_core::services::embedding::WorkspaceEmbeddingResolver,
                >);
        Ok(ctx)
    }

    /// `dashboard` and the Stripe webhook are always mounted (not licence-gated); `marketplace` 404s without a licence.
    /// `inference`'s `/hosted` prefix is an unchecked deviation from `origin`/`entity_columns`, which mount at master's own paths after confirming no collision: reconcile before the PR.
    fn routes(ctx: &AppContext) -> AppRoutes {
        App::routes(ctx)
            .add_route(controllers::dashboard::routes())
            .add_route(controllers::embedding::routes())
            .add_route(controllers::entity_columns::routes())
            .add_route(controllers::inference::routes())
            .add_route(controllers::marketplace::routes())
            .add_route(controllers::oauth::routes())
            .add_route(controllers::origin::routes())
            .add_route(controllers::stripe::routes())
    }

    /// `controllers::mcp::mount` hardcodes a concrete `ServerHandler` type, so an ee-only MCP tool would mean re-implementing `mount` and its layers here.
    async fn after_routes(router: axum::Router, ctx: &AppContext) -> Result<axum::Router> {
        App::after_routes(router, ctx).await
    }

    async fn connect_workers(ctx: &AppContext, queue: &Queue) -> Result<()> {
        App::connect_workers(ctx, queue).await
    }

    fn register_tasks(tasks: &mut Tasks) {
        App::register_tasks(tasks);
        tasks.register(tasks::seed_official_templates::SeedOfficialTemplates);
        tasks.register(tasks::create_tenant_api_key::CreateTenantApiKey);
    }

    async fn truncate(ctx: &AppContext) -> Result<()> {
        App::truncate(ctx).await
    }

    /// Publishing the templates themselves stays `seed_official_templates`'s own job.
    async fn seed(ctx: &AppContext, base: &Path) -> Result<()> {
        App::seed(ctx, base).await?;
        services::official_templates::ensure_official_tenant(&ctx.db).await?;
        Ok(())
    }
}
