//! Re-embeds workspaces whose stored vectors came from a different model than the one now configured.
//!
//! Only a worker process knows the configured model: the API and MCP server holds no provider.
//! So the scan runs once when a worker starts, after its provider is installed and its workers are registered, and it only enqueues durable reindex jobs.
//!
//! Several worker replicas start at about the same time, so the scan is coordinated twice.
//! On PostgreSQL one replica at a time holds a transaction-scoped advisory lock for the whole scan, and a replica that cannot take it skips the scan because another is running it.
//! The lifecycle insert is also protected by a backend-parity unique admission index, so separate processes cannot both record an active reindex.

use loco_rs::app::AppContext;
use sea_orm::TransactionTrait;

/// The advisory lock key every replica contends on.
const SCAN_LOCK: &str = "startup-reindex-scan";

/// Runs the scan once.
/// A failure is logged and never stops the worker from starting: a missed scan is repaired by the next start or by an explicit reindex.
pub async fn run(ctx: &AppContext) {
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
            tracing::error!(error = %err, "startup reindex: failed to list workspaces");
            return;
        }
    };

    for workspace in workspaces {
        let provider = match crate::models::workspace_workspaces::embedding_provider(
            ctx,
            workspace.id,
        )
        .await
        {
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
        if provider.availability()
            == crate::services::embedding::EmbeddingProviderAvailability::Disabled
        {
            tracing::warn!(
                workspace_id = %workspace.id,
                "startup reindex: embedding provider must be configured"
            );
            continue;
        }
        let provider_model = provider.model_name();
        if !workspace.is_stamped_with_other_embedding(&provider_model, provider.dimensions()) {
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

/// Scans persisted stamps using provider metadata without constructing providers.
///
/// # Errors
/// Returns an error if workspace enumeration or metadata resolution fails.
pub async fn run_metadata_scan(ctx: &AppContext) -> loco_rs::Result<()> {
    let settings = ctx
        .config
        .settings::<crate::data::settings::Settings>()
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    let workspaces = crate::models::workspace_workspaces::stamped_for_reindex(&ctx.db)
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    let mut failures = Vec::new();
    for workspace in workspaces {
        let metadata = match resolve_workspace_metadata(ctx, workspace.id, &settings).await {
            Ok(metadata) => metadata,
            Err(error) => {
                failures.push(format!("workspace {}: {error}", workspace.id));
                continue;
            }
        };
        let Some(metadata) = metadata else { continue };
        if workspace.is_stamped_with_other_embedding(&metadata.model, metadata.dimensions)
            && let Err(error) = enqueue_metadata_reindex(ctx, workspace.id).await
        {
            failures.push(format!("workspace {}: {error}", workspace.id));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(loco_rs::Error::Message(format!(
            "startup reindex metadata scan failed: {}",
            failures.join("; ")
        )))
    }
}

async fn resolve_workspace_metadata(
    ctx: &AppContext,
    workspace_id: uuid::Uuid,
    settings: &crate::data::settings::Settings,
) -> Result<Option<crate::services::embedding::EmbeddingMetadata>, crate::YorishiroError> {
    crate::models::workspace_workspaces::effective_embedding_metadata(ctx, workspace_id, settings)
        .await
}

async fn enqueue_reindex(ctx: &AppContext, workspace_id: uuid::Uuid) {
    let worker_class =
        match crate::controllers::extractors::resolve_worker_class(ctx, workspace_id).await {
            Ok(class) => class,
            Err(err) => {
                tracing::warn!(
                    %workspace_id,
                    error = %err.0,
                    "startup reindex: failed to resolve worker class, skipping workspace"
                );
                return;
            }
        };
    let args = crate::workers::reindex::ReindexArgs {
        lifecycle_id: None,
        workspace_id,
        worker_class,
        startup: true,
    };
    match crate::workers::reindex::enqueue_for_class(ctx, args).await {
        Ok(()) => tracing::info!(%workspace_id, "startup reindex: enqueue success"),
        Err(err) => {
            tracing::error!(%workspace_id, error = %err, "startup reindex: failed to enqueue reindex");
        }
    }
}

async fn enqueue_metadata_reindex(
    ctx: &AppContext,
    workspace_id: uuid::Uuid,
) -> loco_rs::Result<()> {
    if active_startup_reindex_exists(ctx, workspace_id).await? {
        return Ok(());
    }
    let worker_class = crate::controllers::extractors::resolve_worker_class(ctx, workspace_id)
        .await
        .map_err(|error| {
            loco_rs::Error::Message(format!("workspace {workspace_id}: {}", error.0))
        })?;
    let args = crate::workers::reindex::ReindexArgs {
        lifecycle_id: None,
        workspace_id,
        worker_class,
        startup: true,
    };
    // A concurrent scan can win the unique admission after this precheck; the next tick observes it.
    crate::workers::reindex::enqueue_for_class(ctx, args)
        .await
        .map_err(|error| loco_rs::Error::Message(format!("workspace {workspace_id}: {error}")))
}

async fn active_startup_reindex_exists(
    ctx: &AppContext,
    workspace_id: uuid::Uuid,
) -> loco_rs::Result<bool> {
    let tenant = crate::models::workspace_workspaces::get_workspace_tenant(&ctx.db, workspace_id)
        .await
        .map_err(|error| loco_rs::Error::Message(format!("workspace {workspace_id}: {error}")))?;
    let txn = crate::db::begin_workspace(ctx, tenant, workspace_id)
        .await
        .map_err(|error| loco_rs::Error::Message(format!("workspace {workspace_id}: {error}")))?;
    let exists = crate::models::queue_job_lifecycles::Entity::has_active(
        &txn,
        crate::workers::reindex::ReindexArgs::STARTUP_JOB_NAME,
        workspace_id,
    )
    .await
    .map_err(|error| loco_rs::Error::Message(format!("workspace {workspace_id}: {error}")))?;
    txn.commit()
        .await
        .map_err(|error| loco_rs::Error::Message(format!("workspace {workspace_id}: {error}")))?;
    Ok(exists)
}
