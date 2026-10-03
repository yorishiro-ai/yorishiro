use std::sync::Arc;

use loco_rs::{
    Result,
    app::AppContext,
    boot::{BootResult, StartMode},
    environment::Environment,
};
use tokio::task::{JoinHandle, spawn};

use crate::db::AppContextBackend;
use crate::workers::embedding_sync::WorkerClass;

pub(super) struct LoadGuardHandle(JoinHandle<()>);

impl LoadGuardHandle {
    async fn shutdown(self) {
        self.0.abort();
        let _ = self.0.await;
    }
}

/// Registers SQLite extensions before any test-harness connection is opened.
pub(super) fn register_sqlite_extensions() {
    crate::db::register_sqlite_extensions();
}

/// Whether this process answers HTTP requests.
/// Deployment-wide startup work (the reindex scan and the load monitor) belongs to the processes users reach, not to queue workers.
pub(super) fn serves_http(mode: &StartMode) -> bool {
    !matches!(
        mode,
        StartMode::WorkerOnly { .. } | StartMode::WorkerAndScheduler { .. }
    )
}

/// Runs the deployment-wide startup work once migrations have applied.
/// Test boots skip it: the scan and the monitor would outlive the test's own database.
pub(super) async fn after_boot(
    result: &BootResult,
    environment: &Environment,
    serves_http: bool,
) -> Result<()> {
    if matches!(environment, Environment::Test) || !serves_http {
        return Ok(());
    }
    let ctx = &result.app_context;
    detect_startup_reindex(ctx).await;

    let settings = ctx
        .shared_store
        .get::<crate::data::settings::Settings>()
        .ok_or_else(|| loco_rs::Error::Message("application settings were not installed".into()))?;
    if let Some(config) =
        crate::services::db_load_guard::LoadGuardConfig::from_settings(&settings.db_load_guard)
    {
        let ctx_for_task = ctx.clone();
        let task =
            spawn(async move { crate::services::db_load_guard::run(ctx_for_task, config).await });
        ctx.shared_store.insert(LoadGuardHandle(task));
    }
    Ok(())
}

pub(super) async fn shutdown(ctx: &AppContext) {
    if let Some(handle) = ctx.shared_store.remove::<LoadGuardHandle>() {
        handle.shutdown().await;
    }
}

/// Performs backend checks that must happen before shared services are constructed.
pub(super) fn validate_backend(ctx: &AppContext) -> Result<()> {
    if ctx.is_sqlite() {
        crate::db::require_min_sqlite_connections(ctx.config.database.max_connections)
            .map_err(loco_rs::Error::Message)?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::serves_http;
    use loco_rs::boot::StartMode;

    #[test]
    fn only_a_differently_stamped_workspace_needs_a_reindex() {
        use crate::models::workspace_workspaces::StartupReindexRow;

        let row = |model: Option<&str>| StartupReindexRow {
            id: uuid::Uuid::nil(),
            embedding_model: model.map(str::to_owned),
        };
        assert!(row(Some("old-model")).is_stamped_with_other_model("new-model"));
        assert!(!row(Some("new-model")).is_stamped_with_other_model("new-model"));
        // An unstamped workspace has no vectors to replace.
        assert!(!row(None).is_stamped_with_other_model("new-model"));
    }

    #[test]
    fn worker_only_modes_do_not_serve_http() {
        assert!(!serves_http(&StartMode::WorkerOnly { tags: vec![] }));
        assert!(!serves_http(&StartMode::WorkerAndScheduler {
            tags: vec![]
        }));
        assert!(serves_http(&StartMode::ServerOnly));
        assert!(serves_http(&StartMode::ServerAndWorker));
    }
}
