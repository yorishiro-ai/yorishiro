//! Re-embeds workspaces whose stored vectors came from a different model than the one now configured.
//!
//! Only a worker process knows the configured model: the API and MCP server holds no provider.
//! So the scan runs once when a worker starts, after its provider is installed and its workers are registered, and it only enqueues durable reindex jobs.
//!
//! Several worker replicas start at about the same time, so the scan is coordinated twice.
//! On PostgreSQL one replica at a time holds a transaction-scoped advisory lock for the whole scan, and a replica that cannot take it skips the scan because another is running it.
//! The lifecycle insert is also protected by a backend-parity unique admission index, so separate processes cannot both record an active reindex.

use loco_rs::app::AppContext;
use loco_rs::environment::Environment;
use sea_orm::TransactionTrait;

use crate::workers::embedding_sync::{WorkerClass, resolve_worker_provider};

/// The advisory lock key every replica contends on.
const SCAN_LOCK: &str = "startup-reindex-scan";

/// Runs the scan once.
/// A failure is logged and never stops the worker from starting: a missed scan is repaired by the next start or by an explicit reindex.
pub async fn run(ctx: &AppContext) {
    // Test boots skip the scan: it would outlive the test's own database.
    if matches!(ctx.environment, Environment::Test) {
        return;
    }
    // Held to the end of this function, so the lock covers the whole scan and is released by the commit or by dropping the transaction.
    let guard = match ctx.db.begin().await {
        Ok(txn) => txn,
        Err(error) => {
            tracing::error!(error = %error, "startup reindex: could not open a transaction for the scan lock");
            return;
        }
    };
    match crate::db::try_lock_for_update(&guard, SCAN_LOCK).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::info!("startup reindex: another worker is scanning, skipping");
            return;
        }
        Err(error) => {
            tracing::error!(error = %error, "startup reindex: could not take the scan lock");
            return;
        }
    }
    detect_startup_reindex(ctx).await;
    if let Err(error) = guard.commit().await {
        tracing::warn!(error = %error, "startup reindex: could not release the scan lock cleanly");
    }
}

/// Detects and enqueues startup reindex for any workspace
/// whose stored vectors were embedded with a model that differs from the current provider.
///
/// Awaiting the finite scan avoids a process-local task while the actual reindex remains non-blocking and retryable through the configured queue provider.
async fn detect_startup_reindex(ctx: &AppContext) {
    let workspaces = match crate::models::workspace_workspaces::stamped_for_reindex(&ctx.db).await {
        Ok(workspaces) => workspaces,
        Err(err) => {
            tracing::error!("startup reindex: failed to list workspaces: {err}");
            return;
        }
    };

    for workspace in workspaces {
        let provider = match resolve_worker_provider(ctx, workspace.id).await {
            Ok(Some(provider)) => provider,
            Ok(None) => {
                tracing::warn!(
                    workspace_id = %workspace.id,
                    "startup reindex: this worker has no embedding provider"
                );
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    workspace_id = %workspace.id,
                    error = %err,
                    "startup reindex: failed to resolve embedding provider"
                );
                continue;
            }
        };
        if provider.embed_batch(&[]).await.is_err() {
            tracing::warn!(
                workspace_id = %workspace.id,
                "startup reindex: embedding provider must be configured"
            );
            continue;
        }
        let provider_model = provider.model_name();
        if !workspace.is_stamped_with_other_model(&provider_model) {
            continue;
        }
        tracing::info!(
            workspace_id = %workspace.id,
            stamped_model = ?workspace.embedding_model,
            %provider_model,
            "startup reindex: model mismatch, enqueueing reindex"
        );
        enqueue_reindex(ctx, workspace.id).await;
    }
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
