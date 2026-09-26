use std::sync::Arc;

use loco_rs::{Result, app::AppContext};

use crate::db::AppContextBackend;

use super::startup;

/// Builds the base application's shared services without moving work past Loco's migration boundary.
pub(super) async fn build(ctx: AppContext) -> Result<AppContext> {
    startup::validate_backend(&ctx)?;

    if ctx.is_postgres() {
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
            .map_err(|e| loco_rs::Error::Message(format!("failed to build identity pool: {e}")))?;
        ctx.shared_store
            .insert(crate::db::DbHandle { tenant, identity });
        // Each of the four trait objects below is replaced by a later `shared_store.insert`:
        // `Arc<dyn Trait>` is keyed by `TypeId`, so the later insert wins without changing
        // any call site.
        ctx.shared_store
            .insert(crate::services::auth::default_authenticator());
    }

    // Boot fails loudly if the embedding provider is misconfigured, rather than deferring the
    // error to the first search.
    let embedding_provider = crate::services::embedding::build_embedding_provider()
        .await
        .map_err(|e| loco_rs::Error::Message(format!("failed to build embedding provider: {e}")))?;
    ctx.shared_store.insert(embedding_provider);
    // Both resolver trait objects are installed on every backend, unlike the authenticator above: they
    // read `ctx.db` directly, and a per-workspace assignment is not an RLS concept.
    ctx.shared_store
        .insert(crate::services::embedding::default_embedding_resolver());
    ctx.shared_store
        .insert(crate::workers::embedding_sync::default_worker_class_resolver());
    // Per-workspace search token budget: a request scope, so it belongs in shared_store rather than being built fresh in after_routes like the (per-IP, request-scoped-only) auth rate limiter is.
    ctx.shared_store.insert(Arc::new(
        crate::services::rate_limit::RateLimiter::search_tokens_from_env(),
    ));

    Ok(ctx)
}
