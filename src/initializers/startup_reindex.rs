//! Re-embeds workspaces whose stored vectors came from a different model than the one now configured.

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router as AxumRouter;
use loco_rs::{
    Result,
    app::{AppContext, Initializer},
    environment::Environment,
};

use crate::workers::embedding_sync::WorkerClass;

/// Runs the scan once, in every process that serves HTTP.
///
/// `after_routes` runs only for start modes that build a router, so queue workers never scan; test boots skip it because the scan would outlive the test's own database.
pub(crate) struct StartupReindex;

#[async_trait]
impl Initializer for StartupReindex {
    fn name(&self) -> String {
        "startup_reindex".into()
    }

    async fn after_routes(&self, router: AxumRouter, ctx: &AppContext) -> Result<AxumRouter> {
        if !matches!(ctx.environment, Environment::Test) {
            detect_startup_reindex(ctx).await;
        }
        Ok(router)
    }
}

/// Detects and enqueues startup reindex for any workspace
/// whose stored vectors were embedded with a model that differs from the current provider.
///
/// This runs once after migrations have applied and only enqueues durable Loco jobs.
/// Awaiting the finite scan avoids a process-local task while the actual reindex remains
/// non-blocking and retryable through the configured queue provider.
///
/// **Community edition only.** This feature compares every workspace's stamped model
/// against the deployment-wide provider. Under EE a workspace can carry its own assignment
/// (see `ee::services::embedding_resolver`), so the comparison would flag every workspace
/// as a mismatch and reindex them with the wrong provider. Skip when a licence is active.
async fn detect_startup_reindex(ctx: &AppContext) {
    // CE-only: under EE per-workspace provider assignment makes this comparison invalid.
    if crate::services::edition::is_active(ctx) {
        tracing::debug!("startup reindex: enterprise licence active, skipping");
        return;
    }
    let Some(provider) = usable_provider(ctx).await else {
        return;
    };
    let workspaces = match crate::models::workspace_workspaces::stamped_for_reindex(&ctx.db).await {
        Ok(workspaces) => workspaces,
        Err(err) => {
            tracing::error!("startup reindex: failed to list workspaces: {err}");
            return;
        }
    };

    let provider_model = provider.model_name();
    for workspace in workspaces
        .iter()
        .filter(|workspace| workspace.is_stamped_with_other_model(&provider_model))
    {
        tracing::info!(
            workspace_id = %workspace.id,
            stamped_model = ?workspace.embedding_model,
            %provider_model,
            "startup reindex: model mismatch, enqueueing reindex"
        );
        enqueue_reindex(ctx, workspace.id).await;
    }
}

/// The deployment's embedding provider, if it is installed and answers.
/// An unconfigured provider accepts the dimension count but fails every call, so it is probed once here instead of failing once per workspace.
async fn usable_provider(
    ctx: &AppContext,
) -> Option<Arc<dyn crate::services::embedding::EmbeddingProvider>> {
    let Some(provider) = ctx
        .shared_store
        .get::<Arc<dyn crate::services::embedding::EmbeddingProvider>>()
    else {
        tracing::warn!("startup reindex: embedding provider missing");
        return None;
    };
    if provider.embed_batch(&[]).await.is_err() {
        tracing::warn!("startup reindex: embedding provider must be configured");
        return None;
    }
    Some(provider)
}

async fn enqueue_reindex(ctx: &AppContext, workspace_id: uuid::Uuid) {
    let worker_class =
        match crate::controllers::extractors::resolve_worker_class(ctx, workspace_id).await {
            Ok(class) => class,
            Err(err) => {
                tracing::warn!(
                    %workspace_id,
                    error = %err.0,
                    "startup reindex: failed to resolve worker class, defaulting to shared"
                );
                WorkerClass::Shared
            }
        };
    let args = crate::workers::reindex::ReindexArgs {
        lifecycle_id: None,
        workspace_id,
        worker_class,
    };
    match crate::workers::reindex::enqueue_for_class(ctx, args).await {
        Ok(()) => tracing::info!(%workspace_id, "startup reindex: enqueue success"),
        Err(err) => {
            tracing::error!(%workspace_id, error = %err, "startup reindex: failed to enqueue reindex");
        }
    }
}
