use std::sync::{Arc, Mutex};

use loco_rs::{Result, app::AppContext, boot::BootResult, environment::Environment};
use tokio::task::{JoinHandle, spawn};

use crate::db::AppContextBackend;
use crate::workers::embedding_sync::WorkerClass;

/// A handle for the startup reindex background task, stored in `shared_store` so that
/// test teardown can signal shutdown and await the task before closing pools.
///
/// Without this, `close_app_pools` would close pools while the spawned task still held
/// a connection from `ctx.db`, leaving a session on the throwaway test database and
/// causing `DROP DATABASE` to panic with "being accessed by other users".
#[derive(Clone)]
pub struct StartupReindexHandle {
    shut: Arc<std::sync::atomic::AtomicBool>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
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

/// Registers SQLite extensions before any test-harness connection is opened.
pub(super) fn register_sqlite_extensions() {
    crate::db::register_sqlite_extensions();
}

/// Runs post-migration startup work without changing the boot hook order.
pub(super) fn after_boot(result: &BootResult, environment: &Environment) {
    if !matches!(environment, Environment::Test) {
        spawn_startup_reindex(result.app_context.clone());
        if let Some(config) = crate::services::db_load_guard::LoadGuardConfig::from_env() {
            let ctx = result.app_context.clone();
            spawn(async move { crate::services::db_load_guard::run(ctx, config).await });
        }
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

/// Spawns a background task that detects and enqueues startup reindex for any workspace
/// whose stored vectors were embedded with a model that differs from the current provider.
///
/// This runs after migrations have applied (see `boot` above), and in a spawned task so the
/// server stays responsive while the check runs. On SQLite there is no embedding column,
/// so this is a no-op.
///
/// **Community edition only.** This feature compares every workspace's stamped model
/// against the deployment-wide provider. Under EE a workspace can carry its own assignment
/// (see `ee::services::embedding_resolver`), so the comparison would flag every workspace
/// as a mismatch and reindex them with the wrong provider. Skip when a licence is active.
fn spawn_startup_reindex(ctx: AppContext) {
    let shut = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Clone before move into the spawned task.
    let shut_for_task = shut.clone();

    let task = Arc::new(Mutex::new(None::<JoinHandle<()>>));
    let task_for_clone = task.clone();

    let handle = StartupReindexHandle {
        shut,
        task: task_for_clone,
    };
    ctx.shared_store.insert(handle);

    let join_handle = spawn(async move {
        // Check for shutdown between each await point.
        // The pattern: do work, sleep briefly (checking shut each iteration), do more work.
        // This ensures the task exits promptly even when blocked on an async op,
        // because the sleep loop always checks the flag.
        let mut do_work = true;
        while do_work {
            do_work = false;

            // CE-only: under EE per-workspace provider assignment makes this comparison invalid.
            if crate::services::edition::is_active(&ctx) {
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

            let workspaces: Vec<_> = match crate::models::workspace_workspaces::Entity::find()
                .select_only()
                .column(crate::models::_entities::workspace_workspaces::Column::Id)
                .column(crate::models::_entities::workspace_workspaces::Column::EmbeddingModel)
                .column(crate::models::_entities::workspace_workspaces::Column::EmbeddingDimensions)
                .column_as(
                    crate::models::_entities::tenant_tenants::Column::EmbeddingModel,
                    "tenant_model",
                )
                .column_as(
                    crate::models::_entities::tenant_tenants::Column::EmbeddingDimensions,
                    "tenant_dimensions",
                )
                .left_join(crate::models::tenant_tenants::Entity)
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
                    // No stamp, no reindex needed. First-write stamping will handle it.
                    continue;
                };

                if stamped_model.as_str() == provider.model_name() {
                    // Already matches, no reindex needed.
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
                let args = crate::workers::reindex::ReindexArgs {
                    workspace_id: ws.id,
                    worker_class,
                };
                if let Err(err) = crate::workers::reindex::enqueue_for_class(&ctx, args).await {
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
