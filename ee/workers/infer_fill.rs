//! Background worker for async infer-fill operations.
//!
//! `POST /api/schemas/active/{name}/infer-fill` enqueues this worker via Loco's queue.
//! The worker processes entities one-by-one, calling the LLM to propose missing field
//! values and writing accepted guesses to `entity_entities`.
//!
//! An advisory lock (`db::lock_for_update`) serializes one infer-fill per workspace,
//! matching the pattern `reindex_embeddings` already uses.

use async_trait::async_trait;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::db::{self, DbHandle};
use crate::ee::models::entity_fill;
use crate::ee::models::llm_keys;
use crate::ee::services::inference::InferenceClient;
use crate::error::ResultExt;
use crate::models::schema_schemas;

/// Arguments for an infer-fill worker job.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InferFillArgs {
    pub workspace_id: Uuid,
    pub schema_name: String,
}

/// Result of an infer-fill job, stored in `shared_store` so the polling endpoint
/// can report applied/skipped counts without querying the queue (Loco's `Queue`
/// does not expose `get_jobs` publicly).
#[derive(Clone, Debug, Serialize)]
pub struct InferFillResult {
    pub workspace_id: Uuid,
    pub applied: i64,
    pub skipped: i64,
    pub error: Option<String>,
    pub completed: bool,
}

/// Thread-safe result tracker stored in `shared_store`.
#[derive(Clone, Default, Debug)]
pub struct ResultTracker {
    results: Arc<Mutex<std::collections::HashMap<String, InferFillResult>>>,
}

impl ResultTracker {
    pub async fn get(&self, job_id: &str) -> Option<InferFillResult> {
        self.results.lock().await.get(job_id).cloned()
    }

    pub async fn set(&self, job_id: String, result: InferFillResult) {
        self.results.lock().await.insert(job_id, result);
    }
}

/// Shared implementation of the infer-fill worker's perform body.
async fn perform_infer_fill(
    ctx: &AppContext,
    args: &InferFillArgs,
    job_id: Uuid,
) -> loco_rs::Result<(i64, i64)> {
    let config = llm_keys::get(&ctx.db, args.workspace_id)
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

    let rows = entity_fill::entities_on_outdated_schema(
        &schema_txn,
        args.workspace_id,
        &args.schema_name,
        active.id,
    )
    .await
    .internal()?;

    let client = InferenceClient::new(config);

    let mut applied = 0i64;
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

        let ok = entity_fill::apply_answers(
            &schema_txn,
            args.workspace_id,
            &entity_fill::OutdatedEntity {
                id: row.id,
                entity_type: row.entity_type.clone(),
                data: row.data.clone(),
            },
            job_id,
            answers,
        )
        .await
        .map_err(|e| loco_rs::Error::Message(e.to_string()))?;

        if ok {
            applied += 1;
        } else {
            skipped += 1;
        }
    }

    schema_txn.commit().await.internal()?;
    Ok((applied, skipped))
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
        vec!["infer-fill".to_string()]
    }

    async fn perform(&self, args: InferFillArgs) -> loco_rs::Result<()> {
        let job_id = Uuid::new_v4();
        let tracker = self
            .ctx
            .shared_store
            .get::<ResultTracker>()
            .expect("ResultTracker should be installed in shared_store")
            .clone();

        // Report queued immediately.
        tracker
            .set(
                job_id.to_string(),
                InferFillResult {
                    workspace_id: args.workspace_id,
                    applied: 0,
                    skipped: 0,
                    error: None,
                    completed: false,
                },
            )
            .await;

        match perform_infer_fill(&self.ctx, &args, job_id).await {
            Ok((applied, skipped)) => {
                tracker
                    .set(
                        job_id.to_string(),
                        InferFillResult {
                            workspace_id: args.workspace_id,
                            applied,
                            skipped,
                            error: None,
                            completed: true,
                        },
                    )
                    .await;
                Ok(())
            }
            Err(e) => {
                tracker
                    .set(
                        job_id.to_string(),
                        InferFillResult {
                            workspace_id: args.workspace_id,
                            applied: 0,
                            skipped: 0,
                            error: Some(e.to_string()),
                            completed: true,
                        },
                    )
                    .await;
                Err(e)
            }
        }
    }
}

/// Enqueue an infer-fill job for the given workspace and schema name.
///
/// Returns the job ID assigned by the queue provider.
///
/// # Errors
/// Returns `loco_rs::Error` when the queue is configured but the enqueue fails.
pub async fn enqueue_infer_fill(
    ctx: &AppContext,
    workspace_id: Uuid,
    schema_name: String,
) -> loco_rs::Result<String> {
    let args = InferFillArgs {
        workspace_id,
        schema_name,
    };

    if ctx.queue_provider.is_none() {
        return Err(loco_rs::Error::Message(
            "infer-fill requires a queue provider (configure queue: in the server config)".into(),
        ));
    }

    let job_id = InferFillWorker::perform_later(ctx, args)
        .await
        .ok()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    Ok(job_id)
}
