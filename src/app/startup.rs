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

/// Selects whether post-migration startup work applies to this process mode.
pub(super) fn should_run_startup_reindex(mode: &StartMode) -> bool {
    !matches!(
        mode,
        StartMode::WorkerOnly { .. } | StartMode::WorkerAndScheduler { .. }
    )
}

pub(super) async fn after_boot(
    result: &BootResult,
    environment: &Environment,
    run_startup_reindex: bool,
) {
    if !matches!(environment, Environment::Test) && run_startup_reindex {
        detect_startup_reindex(&result.app_context).await;
        let settings = result
            .app_context
            .shared_store
            .get::<crate::config::Settings>()
            .expect("application settings were installed in after_context");
        if let Some(config) =
            crate::services::db_load_guard::LoadGuardConfig::from_settings(&settings)
        {
            let ctx = result.app_context.clone();
            let task = spawn(async move { crate::services::db_load_guard::run(ctx, config).await });
            result
                .app_context
                .shared_store
                .insert(LoadGuardHandle(task));
        }
    }
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

    let Some(provider) = ctx
        .shared_store
        .get::<Arc<dyn crate::services::embedding::EmbeddingProvider>>()
    else {
        tracing::warn!("startup reindex: embedding provider missing");
        return;
    };
    if provider.embed_batch(&[]).await.is_err() {
        tracing::warn!("startup reindex: embedding provider must be configured");
        return;
    }

    let workspaces = match crate::models::workspace_workspaces::stamped_for_reindex(&ctx.db).await {
        Ok(workspaces) => workspaces,
        Err(err) => {
            tracing::error!("startup reindex: failed to list workspaces: {err}");
            return;
        }
    };

    for workspace in workspaces {
        let Some(stamped_model) = &workspace.embedding_model else {
            continue;
        };
        if stamped_model.as_str() == provider.model_name() {
            continue;
        }

        let worker_class =
            match crate::controllers::extractors::resolve_worker_class(ctx, workspace.id).await {
                Ok(class) => class,
                Err(err) => {
                    tracing::warn!(
                        workspace_id = %workspace.id,
                        error = %err.0,
                        "startup reindex: failed to resolve worker class, defaulting to shared"
                    );
                    WorkerClass::Shared
                }
            };
        let args = crate::workers::reindex::ReindexArgs {
            lifecycle_id: None,
            workspace_id: workspace.id,
            worker_class,
        };
        if let Err(err) = crate::workers::reindex::enqueue_for_class(ctx, args).await {
            tracing::error!(workspace_id = %workspace.id, error = %err, "startup reindex: failed to enqueue reindex");
        } else {
            tracing::info!(workspace_id = %workspace.id, "startup reindex: enqueue success");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::should_run_startup_reindex;
    use loco_rs::boot::StartMode;

    #[test]
    fn worker_only_modes_skip_startup_reindex() {
        assert!(!should_run_startup_reindex(&StartMode::WorkerOnly {
            tags: vec![]
        }));
        assert!(!should_run_startup_reindex(
            &StartMode::WorkerAndScheduler { tags: vec![] }
        ));
        assert!(should_run_startup_reindex(&StartMode::ServerOnly));
        assert!(should_run_startup_reindex(&StartMode::ServerAndWorker));
    }
}
