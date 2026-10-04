//! Background worker for async infer-fill operations.
//!
//! `POST /api/schemas/active/{name}/infer-fill` enqueues this worker via Loco's queue.
//! The worker processes entities one-by-one, calling the LLM to propose missing field
//! values and storing them as reviewable proposals.
//!
//! An advisory lock (`db::lock_for_update`) serializes one infer-fill per workspace,
//! matching the pattern `reindex_embeddings` already uses.

use async_trait::async_trait;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::{self, DbHandle};
use crate::ee::models::entity_entities::infer_fill;
use crate::ee::models::inference_jobs;
use crate::ee::models::inference_proposals;
use crate::ee::models::workspace_llm_keys;
use crate::ee::services::inference::InferenceClient;
use crate::error::ResultExt;
use crate::models::schema_schemas;

/// The tag infer-fill jobs carry, and the named queue they use on providers that honour one.
pub(crate) const QUEUE: &str = "infer-fill";

/// Arguments for an infer-fill worker job.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InferFillArgs {
    #[serde(default)]
    pub(crate) lifecycle_id: Option<Uuid>,
    pub job_id: Uuid,
    pub workspace_id: Uuid,
    pub schema_name: String,
}

impl InferFillArgs {
    /// The name this job is recorded under in the queue lifecycle.
    pub(crate) const JOB_NAME: &'static str = "infer_fill";
}

/// The dispatch seam for infer-fill jobs.
///
/// `compose_context` stores the production adapter as `Arc<dyn InferFillDispatcher>` in `AppContext::shared_store`.
/// A caller that needs a different dispatcher inserts its own `Arc<dyn InferFillDispatcher>` there afterwards.
#[async_trait]
pub trait InferFillDispatcher: Send + Sync {
    async fn dispatch(&self, ctx: &AppContext, args: InferFillArgs) -> loco_rs::Result<String>;
}

/// Shared implementation of the infer-fill worker's perform body.
async fn perform_infer_fill(
    ctx: &AppContext,
    args: &InferFillArgs,
    job_id: Uuid,
    attempt: Option<i32>,
) -> loco_rs::Result<(i64, i64)> {
    let config = workspace_llm_keys::get(&ctx.db, args.workspace_id)
        .await
        .internal()?
        .ok_or_else(|| {
            loco_rs::Error::Message("workspace has no LLM credentials configured".into())
        })?;

    let db_handle = ctx.shared_store.get::<DbHandle>().ok_or_else(|| {
        loco_rs::Error::Message(
            "infer-fill requires the tenant pool, which this deployment did not build".into(),
        )
    })?;
    let schema_txn = db_handle
        .tenant
        .begin_for_workspace(args.workspace_id, args.workspace_id)
        .await
        .internal()?;

    // Serialize one infer-fill per workspace via transaction-scoped advisory lock.
    db::lock_for_update(&schema_txn, &format!("infer-fill:{}", args.workspace_id))
        .await
        .map_err(|e| loco_rs::Error::Message(e.to_string()))?;

    let active =
        schema_schemas::get_active_schema(&schema_txn, args.workspace_id, &args.schema_name)
            .await
            .internal()?;

    let rows = infer_fill::entities_on_outdated_schema(
        &schema_txn,
        args.workspace_id,
        &args.schema_name,
        active.id,
    )
    .await
    .internal()?;

    let client = InferenceClient::new(config);

    let mut proposed = 0i64;
    let mut skipped = 0i64;

    for row in &rows {
        let Some(type_def) = active.definition.entity_types.get(&row.entity_type) else {
            continue;
        };

        let missing: Vec<&str> = type_def
            .fields
            .keys()
            .filter(|field| row.data.get(field.as_str()).is_none())
            .map(|field| field.as_str())
            .collect();

        if missing.is_empty() {
            skipped += 1;
            continue;
        }

        let answers = client
            .propose_fields(&row.data, &missing)
            .await
            .map_err(|e| loco_rs::Error::Message(format!("LLM propose_fields failed: {e}")))?;

        if answers.is_empty() {
            skipped += 1;
            continue;
        }

        let written = inference_proposals::record_batch_attempt(
            &schema_txn,
            args.workspace_id,
            job_id,
            attempt,
            active.id,
            active.version,
            answers
                .into_iter()
                .map(|(field, value)| (row.id, field, value)),
        )
        .await
        .map_err(|e| loco_rs::Error::Message(e.to_string()))?;

        if written == 0 {
            skipped += 1;
        } else {
            proposed += written;
        }
    }

    schema_txn.commit().await.internal()?;
    Ok((proposed, skipped))
}

/// Infer-fill worker.
pub struct InferFillWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<InferFillArgs> for InferFillWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    fn tags() -> Vec<String> {
        vec![QUEUE.to_string()]
    }

    fn queue() -> Option<String> {
        Some(QUEUE.to_string())
    }

    async fn perform(&self, args: InferFillArgs) -> loco_rs::Result<()> {
        let mut already_reconciled = false;
        let has_lifecycle = args.lifecycle_id.is_some();
        let admission = if let Some(id) = args.lifecycle_id {
            match crate::models::queue_job_lifecycles::Entity::start(&self.ctx.db, id).await {
                Ok(
                    admission @ (crate::models::queue_job_lifecycles::Admission::Started { .. }
                    | crate::models::queue_job_lifecycles::Admission::Recovered {
                        ..
                    }),
                ) => admission,
                Ok(crate::models::queue_job_lifecycles::Admission::Duplicate { attempt }) => {
                    if !inference_jobs::reconcile_attempt(&self.ctx.db, args.job_id, attempt)
                        .await
                        .internal()?
                    {
                        return Ok(());
                    }
                    already_reconciled = true;
                    crate::models::queue_job_lifecycles::Admission::Started { attempt }
                }
                Ok(crate::models::queue_job_lifecycles::Admission::Terminal) => return Ok(()),
                Ok(crate::models::queue_job_lifecycles::Admission::Saturated { attempt }) => {
                    match attempt {
                        Some(attempt) => {
                            crate::models::queue_job_lifecycles::Entity::defer_at(
                                &self.ctx.db,
                                id,
                                attempt,
                                "worker capacity saturated",
                                chrono::Utc::now().fixed_offset(),
                            )
                            .await
                        }
                        None => {
                            crate::models::queue_job_lifecycles::Entity::defer(
                                &self.ctx.db,
                                id,
                                None,
                                "worker capacity saturated",
                            )
                            .await
                        }
                    }
                    .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
                    let scheduling = crate::workers::queue::decide(
                        crate::workers::embedding_sync::WorkerClass::Shared,
                    );
                    Self::perform_later_with_priority(
                        &self.ctx,
                        args.clone(),
                        Some(scheduling.priority),
                    )
                    .await?;
                    return Ok(());
                }
                Err(error) => return Err(loco_rs::Error::Message(error.to_string())),
            }
        } else {
            crate::models::queue_job_lifecycles::Admission::Started { attempt: 0 }
        };
        let attempt = match admission {
            crate::models::queue_job_lifecycles::Admission::Started { attempt }
            | crate::models::queue_job_lifecycles::Admission::Recovered { attempt } => {
                Some(attempt)
            }
            crate::models::queue_job_lifecycles::Admission::Duplicate { .. }
            | crate::models::queue_job_lifecycles::Admission::Saturated { .. }
            | crate::models::queue_job_lifecycles::Admission::Terminal => None,
        };
        let admitted = attempt.is_some();
        let job_id = args.job_id;
        let claimed = if already_reconciled {
            Ok(true)
        } else if has_lifecycle {
            let attempt = attempt.expect("lifecycle admission has an attempt");
            inference_jobs::reconcile_attempt(&self.ctx.db, job_id, attempt).await
        } else {
            inference_jobs::claim(&self.ctx.db, job_id).await
        }
        .internal()?;
        if !claimed {
            // A duplicate delivery is harmless after the first worker claims the row.
            // This also makes a reaped delivery harmless while the original worker may
            // still be running.
            return Ok(());
        }

        let attempt = inference_jobs::get(&self.ctx.db, job_id)
            .await
            .internal()?
            .map(|job| job.attempt)
            .or(attempt);
        let Some(attempt) = attempt else {
            return Err(loco_rs::Error::Message(
                "claimed infer-fill job disappeared".into(),
            ));
        };

        let heartbeat = args.lifecycle_id.map(|id| {
            crate::models::queue_job_lifecycles::Entity::heartbeat(self.ctx.db.clone(), id, attempt)
        });

        match perform_infer_fill(&self.ctx, &args, job_id, Some(attempt)).await {
            Ok((proposed, skipped)) => {
                if let Some(heartbeat) = heartbeat {
                    heartbeat.abort();
                }
                let owned = inference_jobs::complete_proposals_attempt(
                    &self.ctx.db,
                    job_id,
                    attempt,
                    proposed,
                    skipped,
                )
                .await
                .internal()?;
                if owned
                    && admitted
                    && let Some(id) = args.lifecycle_id
                {
                    let _ = crate::models::queue_job_lifecycles::Entity::finish(
                        &self.ctx.db,
                        id,
                        Some(attempt),
                        crate::models::queue_job_lifecycles::LifecycleStatus::Completed,
                        None,
                    )
                    .await;
                }
                Ok(())
            }
            Err(e) => {
                if let Some(heartbeat) = heartbeat {
                    heartbeat.abort();
                }
                let owned = match inference_jobs::fail_attempt(
                    &self.ctx.db,
                    job_id,
                    attempt,
                    &e.to_string(),
                )
                .await
                {
                    Ok(owned) => owned,
                    Err(update_error) => {
                        tracing::error!(%job_id, error = %update_error, "failed to persist infer-fill error");
                        false
                    }
                };
                if owned
                    && admitted
                    && let Some(id) = args.lifecycle_id
                {
                    inference_jobs::retry(&self.ctx.db, job_id, &e.to_string(), attempt)
                        .await
                        .internal()?;
                    crate::models::queue_job_lifecycles::Entity::defer(
                        &self.ctx.db,
                        id,
                        Some(attempt),
                        &e.to_string(),
                    )
                    .await
                    .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
                    let scheduling = crate::workers::queue::decide(
                        crate::workers::embedding_sync::WorkerClass::Shared,
                    );
                    Self::perform_later_with_priority(
                        &self.ctx,
                        args.clone(),
                        Some(scheduling.priority),
                    )
                    .await?;
                    return Ok(());
                }
                Err(e)
            }
        }
    }
}

/// Enqueue an infer-fill job for the given workspace and schema name.
///
/// Returns the job ID that the caller can use to poll status.
///
/// # Errors
/// Returns `loco_rs::Error` when the queue is configured but the enqueue fails.
pub async fn enqueue_infer_fill(
    ctx: &AppContext,
    workspace_id: Uuid,
    schema_name: String,
) -> loco_rs::Result<String> {
    if ctx.queue_provider.is_none() {
        return Err(loco_rs::Error::Message(
            "infer-fill requires a queue provider (configure queue: in the server config)".into(),
        ));
    }

    let job_id = Uuid::new_v4();

    inference_jobs::create(&ctx.db, job_id, workspace_id, &schema_name)
        .await
        .internal()?;

    let dispatch_result = match ctx
        .shared_store
        .get::<std::sync::Arc<dyn InferFillDispatcher>>()
    {
        Some(dispatcher) => {
            enqueue_infer_fill_with_dispatcher(
                ctx,
                InferFillArgs {
                    lifecycle_id: None,
                    job_id,
                    workspace_id,
                    schema_name,
                },
                dispatcher.as_ref(),
            )
            .await
        }
        None => Err(loco_rs::Error::Message(
            "InferFillDispatcher missing".into(),
        )),
    };
    if let Err(error) = dispatch_result {
        let message = format!("failed to enqueue infer-fill job: {error}");
        inference_jobs::fail(&ctx.db, job_id, &message)
            .await
            .internal()?;
        return Err(loco_rs::Error::Message(message));
    }

    Ok(job_id.to_string())
}

pub(crate) async fn enqueue_infer_fill_with_dispatcher(
    ctx: &AppContext,
    args: InferFillArgs,
    dispatcher: &dyn InferFillDispatcher,
) -> loco_rs::Result<String> {
    dispatcher.dispatch(ctx, args).await
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct RecordingDispatcher {
        args: Mutex<Vec<InferFillArgs>>,
        fail: bool,
    }

    #[async_trait]
    impl InferFillDispatcher for RecordingDispatcher {
        async fn dispatch(
            &self,
            _ctx: &AppContext,
            args: InferFillArgs,
        ) -> loco_rs::Result<String> {
            self.args.lock().unwrap().push(args);
            if self.fail {
                Err(loco_rs::Error::Message("dispatch failed".into()))
            } else {
                Ok("infer-fill-job".into())
            }
        }
    }

    #[tokio::test]
    async fn dispatches_the_original_args_and_returns_success() {
        let ctx = crate::workers::dispatch::test_context().await;
        let args = InferFillArgs {
            lifecycle_id: None,
            job_id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
            schema_name: "note".into(),
        };
        let dispatcher = RecordingDispatcher {
            args: Mutex::new(Vec::new()),
            fail: false,
        };

        let job_id = enqueue_infer_fill_with_dispatcher(&ctx, args.clone(), &dispatcher)
            .await
            .expect("dispatch");

        assert_eq!(job_id, "infer-fill-job");
        let recorded = dispatcher.args.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].job_id, args.job_id);
        assert_eq!(recorded[0].workspace_id, args.workspace_id);
        assert_eq!(recorded[0].schema_name, args.schema_name);
    }

    #[tokio::test]
    async fn preserves_dispatch_failure() {
        let ctx = crate::workers::dispatch::test_context().await;
        let dispatcher = RecordingDispatcher {
            args: Mutex::new(Vec::new()),
            fail: true,
        };

        let error = enqueue_infer_fill_with_dispatcher(
            &ctx,
            InferFillArgs {
                lifecycle_id: None,
                job_id: Uuid::now_v7(),
                workspace_id: Uuid::now_v7(),
                schema_name: "note".into(),
            },
            &dispatcher,
        )
        .await
        .expect_err("dispatch must fail");

        assert_eq!(error.to_string(), "dispatch failed");
    }

    #[tokio::test]
    async fn rejects_a_missing_queue_before_creating_a_job() {
        let ctx = crate::workers::dispatch::test_context().await;

        let error = enqueue_infer_fill(&ctx, Uuid::now_v7(), "note".into())
            .await
            .expect_err("missing queue must fail");

        assert_eq!(
            error.to_string(),
            "infer-fill requires a queue provider (configure queue: in the server config)"
        );
    }
}
