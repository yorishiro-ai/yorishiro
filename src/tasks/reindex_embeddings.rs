use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::workers::reindex::{self, ReindexArgs};

/// `cargo loco task reindex_embeddings workspace_id:<uuid>`
///
/// Queues a reindex job that re-embeds every entity in a workspace with the embedding provider the worker is configured with, then restamps `workspace_workspaces.embedding_model`/`embedding_dimensions` to that provider's own values, so a workspace can move from one local model to another without the write-time model check refusing every subsequent write forever.
///
/// The command only enqueues, the same as `POST /api/migration-jobs/reindex`.
/// It holds no embedding provider, because only a worker process loads a model, so it cannot compare the workspace's stamp with the model and always queues the job: the worker re-embeds every entity with an `x-embed` field whether or not it already has a vector, since the point is replacing vectors from the old model.
/// A `force` argument is accepted and ignored, so existing runbooks keep working.
///
/// The restamp happens only after every entity embeds successfully, never before and never partially; `entity_embeddings::reindex_workspace` is where that ordering actually lives, and the reindex worker is a thin shell over it.
/// A failure partway through leaves the workspace stamped with its old model, which correctly keeps the write-time check refusing new writes until a reindex succeeds, and re-running is safe.
///
/// PostgreSQL only in practice, for the same reason as the reindex worker: it takes the tenant pool's advisory lock.
pub(crate) struct ReindexEmbeddings;

#[async_trait]
impl Task for ReindexEmbeddings {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "reindex_embeddings".to_string(),
            detail: "Queues a reindex that re-embeds every entity in a workspace and restamps its model: cargo loco task reindex_embeddings workspace_id:<uuid>".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let workspace_id: Uuid = vars.cli_arg("workspace_id")?.parse().map_err(|_| {
            YorishiroError::ValidationFailed {
                message: "workspace_id is not a valid UUID".into(),
                details: vec![],
                hint: "workspace_id must be a UUID, e.g. 00000000-0000-0000-0000-000000000000"
                    .to_string(),
            }
        })?;

        let worker_class =
            crate::controllers::extractors::resolve_worker_class(app_context, workspace_id)
                .await
                .map_err(|error| error.0)?;
        reindex::enqueue_for_class(
            app_context,
            ReindexArgs {
                lifecycle_id: None,
                workspace_id,
                worker_class,
                startup: false,
            },
        )
        .await?;

        println!(
            "reindex queued for workspace {workspace_id}; a worker re-embeds it and restamps its model when every entity succeeds"
        );
        Ok(())
    }
}
