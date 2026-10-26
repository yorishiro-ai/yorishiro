//! Background worker for async infer-fill operations.
//!
//! `POST /api/schemas/active/{name}/infer-fill` enqueues this worker via Loco's queue.
//! The worker processes entities one-by-one, calling the LLM to propose missing field
//! values and storing them as reviewable proposals.
//!
//! A partial unique index on running inference jobs serializes one infer-fill per workspace.
//! Workspace snapshots and proposal writes use short transactions; external LLM calls run outside them.

use loco_rs::prelude::async_trait;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db;
use crate::edition::ee::models::entity_entities::infer_fill;
use crate::edition::ee::models::inference_jobs;
use crate::edition::ee::models::inference_proposals;
use crate::edition::ee::models::workspace_llm_keys;
use crate::edition::ee::services::inference::InferenceClient;
use crate::error::ResultExt;
use crate::models::schema_schemas;

/// Arguments for an infer-fill worker job.
#[derive(Clone, Serialize, Deserialize)]
pub struct InferFillArgs {
    #[serde(default)]
    pub lifecycle_id: Option<Uuid>,
    pub job_id: Uuid,
    pub workspace_id: Uuid,
    pub schema_name: String,
}

impl InferFillArgs {
    /// The name this job is recorded under in the queue lifecycle.
    pub(crate) const JOB_NAME: &'static str = "infer_fill";
}

/// Opens the transaction an infer-fill run reads and writes in, scoped the way a request for this workspace would be.
///
/// The queued arguments carry the workspace only, so the tenant is not known to the worker until it is read from the workspace row.
///
/// # Errors
/// Returns an error if the workspace does not exist or the transaction cannot be started.
pub async fn open_workspace_scope(
    ctx: &AppContext,
    workspace_id: Uuid,
) -> loco_rs::Result<sea_orm::DatabaseTransaction> {
    // `ctx.db` is the identity connection, which reads the workspace row without a tenant scope.
    let workspace =
        crate::models::_entities::workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .internal()?
            .ok_or_else(|| {
                loco_rs::Error::Message(format!("workspace {workspace_id} not found"))
            })?;
    db::begin_workspace(ctx, workspace.tenant_id, workspace_id)
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))
}

async fn open_workspace_write_scope(
    ctx: &AppContext,
    workspace_id: Uuid,
) -> loco_rs::Result<sea_orm::DatabaseTransaction> {
    let tenant_id =
        crate::models::workspace_workspaces::get_workspace_tenant(&ctx.db, workspace_id)
            .await
            .internal()?;
    db::begin_workspace_write(ctx, tenant_id, workspace_id)
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))
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

    let schema_txn = open_workspace_scope(ctx, args.workspace_id).await?;

    // Take a consistent workspace snapshot without holding a database transaction
    // open across the external LLM call. The one-running-job-per-workspace index
    // remains the admission fence on both PostgreSQL and SQLite.
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
    schema_txn.commit().await.internal()?;

    let client = InferenceClient::new(config);

    let mut proposed = 0i64;
    let mut skipped = 0i64;
    let mut batches = Vec::new();

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

        batches.push((row.id, answers));
    }

    if !batches.is_empty() {
        let proposal_txn = open_workspace_write_scope(ctx, args.workspace_id).await?;
        schema_schemas::lock_version(&proposal_txn, args.workspace_id, &args.schema_name)
            .await
            .internal()?;
        let current =
            schema_schemas::get_active_schema(&proposal_txn, args.workspace_id, &args.schema_name)
                .await?;
        if current.id != active.id || current.version != active.version {
            return Err(loco_rs::Error::Message(
                "active schema changed while infer-fill proposals were being generated".into(),
            ));
        }
        for (entity_id, answers) in batches {
            let written = inference_proposals::record_batch_attempt(
                &proposal_txn,
                args.workspace_id,
                job_id,
                attempt,
                active.id,
                active.version,
                answers
                    .into_iter()
                    .map(|(field, value)| (entity_id, field, value)),
            )
            .await
            .map_err(|e| loco_rs::Error::Message(e.to_string()))?;

            if written == 0 {
                skipped += 1;
            } else {
                proposed += written;
            }
        }
        proposal_txn.commit().await.internal()?;
    }

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

    async fn perform(&self, args: InferFillArgs) -> loco_rs::Result<()> {
        self.perform_job(args).await
    }
}

impl InferFillWorker {
    /// Hands a job back to the queue because another job of its workspace is running.
    ///
    /// Without a lifecycle row the run fails and requires explicit retry on the native SQL queue.
    /// With one, the admitted attempt is released and the job is enqueued again at once, so a busy workspace costs one poll interval per redelivery until the running job ends.
    async fn defer_for_busy_workspace(
        &self,
        args: &InferFillArgs,
        attempt: Option<i32>,
    ) -> loco_rs::Result<()> {
        const DIAGNOSTIC: &str = "workspace has a running infer-fill job";
        if args.lifecycle_id.is_none() {
            return Err(loco_rs::Error::Message(DIAGNOSTIC.into()));
        }
        defer_and_enqueue(&self.ctx, args, attempt, DIAGNOSTIC, false).await?;
        Ok(())
    }

    async fn perform_job(&self, args: InferFillArgs) -> loco_rs::Result<()> {
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
                    defer_and_enqueue(
                        &self.ctx,
                        &args,
                        attempt,
                        "worker capacity saturated",
                        false,
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
        let claim = if already_reconciled {
            Ok(true)
        } else if has_lifecycle {
            let attempt = attempt.expect("lifecycle admission has an attempt");
            inference_jobs::reconcile_attempt(&self.ctx.db, job_id, attempt).await
        } else {
            inference_jobs::claim(&self.ctx.db, job_id).await
        };
        let claimed = match claim {
            Ok(claimed) => claimed,
            Err(error) if inference_jobs::is_workspace_busy(&error) => {
                return self.defer_for_busy_workspace(&args, attempt).await;
            }
            Err(error) => return Err(loco_rs::Error::Message(error.to_string())),
        };
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
                if admitted && args.lifecycle_id.is_some() {
                    defer_and_enqueue(&self.ctx, &args, Some(attempt), &e.to_string(), true)
                        .await?;
                    return Ok(());
                }
                inference_jobs::fail_attempt(&self.ctx.db, job_id, attempt, &e.to_string())
                    .await
                    .internal()?;
                Err(e)
            }
        }
    }
}

async fn defer_and_enqueue(
    ctx: &AppContext,
    args: &InferFillArgs,
    attempt: Option<i32>,
    error: &str,
    retry_inference_job: bool,
) -> loco_rs::Result<()> {
    let lifecycle_id = args
        .lifecycle_id
        .ok_or_else(|| loco_rs::Error::Message("infer-fill retry has no lifecycle id".into()))?;
    let tenant_id =
        crate::models::workspace_workspaces::get_workspace_tenant(&ctx.db, args.workspace_id)
            .await
            .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    let txn = crate::db::begin_workspace(ctx, tenant_id, args.workspace_id)
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    if let Some(attempt) = attempt {
        if retry_inference_job {
            inference_jobs::retry(&txn, args.job_id, error, attempt)
                .await
                .internal()?;
        }
        crate::models::queue_job_lifecycles::Entity::defer(
            &txn,
            lifecycle_id,
            Some(attempt),
            error,
        )
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    } else {
        crate::models::queue_job_lifecycles::Entity::defer(&txn, lifecycle_id, None, error)
            .await
            .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    }
    let target = crate::workers::target::WorkerTarget::for_job(
        &ctx.db,
        crate::workers::embedding_sync::WorkerClass::Shared,
        args.workspace_id,
    )
    .await
    .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    let mut payload =
        serde_json::to_value(args).map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    if let serde_json::Value::Object(fields) = &mut payload {
        fields.insert(
            "lifecycle_id".into(),
            serde_json::Value::String(lifecycle_id.to_string()),
        );
    }
    crate::models::queue_job_dispatch_outbox::record_lifecycle_payload(
        &txn,
        lifecycle_id,
        payload,
        InferFillWorker::class_name(),
        Some(target.job_tag()),
        Some(vec![target.job_tag()]),
    )
    .await
    .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    txn.commit()
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))
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
    let tenant_id =
        crate::models::workspace_workspaces::get_workspace_tenant(&ctx.db, workspace_id)
            .await
            .internal()?;
    let txn = crate::db::begin_workspace(ctx, tenant_id, workspace_id)
        .await
        .internal()?;
    inference_jobs::create(&txn, job_id, workspace_id, &schema_name)
        .await
        .internal()?;
    crate::edition::ee::workers::dispatch::dispatch_infer_fill(
        ctx,
        InferFillArgs {
            lifecycle_id: None,
            job_id,
            workspace_id,
            schema_name,
        },
        Some(&txn),
    )
    .await?;
    txn.commit().await.internal()?;

    Ok(job_id.to_string())
}
