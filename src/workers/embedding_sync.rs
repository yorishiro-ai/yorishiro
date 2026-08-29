//! Generates and stores an entity's embedding vector via Loco's own `BackgroundQueue`.
//!
//! A queue provider rather than a bare `tokio::spawn`: a spawned task loses every in-flight sync on a process restart, a forced kill, or a provider outage past its own retry budget, leaving the entity's `embedding` column permanently `NULL` with nothing to retry it (`tasks::resync_embeddings` is the operational recovery command for rows in that state).
//! `pg_loco_queue` persists the job in Postgres, so a re-deployed or restarted process resumes it instead of losing it.
//!
//! **There is no "subscribe to every `WorkerClass`" worker mode.** An empty tag list makes loco's
//! dequeue query `AND (tags IS NULL)` (confirmed against `loco-rs` 1.1.0's `bgworker/pg.rs`, and the
//! matching logic in `sqlt.rs`/`redis.rs`), so a bare `--worker` process dequeues only *untagged*
//! jobs. Every job this module enqueues carries exactly one tag, so such a process takes none of
//! them — not "the leftover ones nothing else claimed".
//!
//! Covering every class in one process means naming every tag:
//! `cargo loco start --worker=worker-class:tenant-private,worker-class:official,worker-class:shared`.
//! There is no wildcard flag, so a new [`WorkerClass`] needs its tag added to those commands by
//! hand. The matching worker type is caught at compile time by `enqueue_for_class`'s exhaustive
//! match; the command is not, and is an operational concern.

use std::sync::Arc;

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::content_entities;
use crate::services::embedding;

/// Which class of worker process a queued job is meant for.
///
/// Routes jobs by `BackgroundWorker::tags()` rather than by named queue: `queue: Option<String>` is
/// silently discarded by the Postgres provider's `enqueue` (it has no column for it), so a
/// named-queue split would mean switching to Redis first.
///
/// `tags()` takes no arguments and is called before a job's own `args` are seen (`loco-rs` 1.1.0's
/// `perform_later_with_priority`), so one worker *type* carries one fixed tag set. A single type
/// tagged with every class would put all three tags on every job, and a `--worker=worker-class:...`
/// process would then dequeue every class's work rather than its own.
///
/// Hence one type per class ([`EmbeddingSyncWorkerTenantPrivate`], [`EmbeddingSyncWorkerOfficial`],
/// [`EmbeddingSyncWorkerShared`]), each fixed to a single tag, so the class picked at enqueue time
/// is the tag that lands in the queue table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerClass {
    /// Runs only on compute a single tenant registered for its own workspaces.
    TenantPrivate,
    /// Runs only on compute this deployment operates itself.
    Official,
    /// Runs on any worker process willing to take the job; the default for a deployment with no registered compute of its own.
    Shared,
}

impl WorkerClass {
    /// The `tags()` value this class routes through.
    ///
    /// `worker-class:<variant>` rather than the bare variant name: a future tag dimension (region, priority band) added to the same job would otherwise collide on an unprefixed string with no way to tell which dimension it came from.
    fn tag(self) -> &'static str {
        match self {
            Self::TenantPrivate => "worker-class:tenant-private",
            Self::Official => "worker-class:official",
            Self::Shared => "worker-class:shared",
        }
    }

    /// The `snake_case` wire form this type already serializes to/from (`#[serde(rename_all = "snake_case")]`), exposed as a plain string for `ee/`'s own persistence (`identity_workspace_worker_classes`).
    /// Storing this same string rather than inventing a second representation means a value read from the database and one read off `EmbeddingSyncArgs`'s own queue payload are byte-identical, so an operator inspecting either sees the same thing.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::TenantPrivate => "tenant_private",
            Self::Official => "official",
            Self::Shared => "shared",
        }
    }

    /// The inverse of [`Self::as_db_str`].
    ///
    /// # Errors
    /// Returns an error if `value` is not one of the three known strings: a row written by a future variant this binary doesn't know about, or a hand-edited/corrupted value.
    pub fn from_db_str(value: &str) -> Result<Self, YorishiroError> {
        match value {
            "tenant_private" => Ok(Self::TenantPrivate),
            "official" => Ok(Self::Official),
            "shared" => Ok(Self::Shared),
            other => Err(YorishiroError::Internal(anyhow::anyhow!(
                "unknown worker_class value: {other:?}"
            ))),
        }
    }
}

/// Resolves a workspace's own worker-class assignment, if it has one.
///
/// A seam, the same shape as [`crate::services::embedding::WorkspaceEmbeddingResolver`]: a deployment can let a workspace pin its embedding-sync jobs to a tenant-private or official-node worker instead of the shared pool, without touching the callers that enqueue a job.
/// [`DefaultWorkerClassResolver`] is the behaviour of every deployment that does not replace it: every workspace's jobs stay `Shared`.
///
/// `conn` is `ctx.db`, not the RLS-scoped tenant pool, for the same reason `WorkspaceEmbeddingResolver` takes it: a per-workspace worker-class assignment is deployment configuration (which compute a tenant pays for), read the same way `identity_workspace_llm_keys`/`identity_workspace_embedding_keys` are, not tenant content.
///
/// Returns `Ok(None)` when the workspace has no assignment of its own, so the caller falls back to [`WorkerClass::Shared`] rather than this seam deciding the fallback itself.
#[async_trait]
pub trait WorkerClassResolver: Send + Sync {
    async fn resolve(
        &self,
        conn: &sea_orm::DatabaseConnection,
        workspace_id: Uuid,
    ) -> Result<Option<WorkerClass>, YorishiroError>;
}

/// This crate's own rule: no workspace has a worker-class assignment, so every job stays `Shared`.
pub struct DefaultWorkerClassResolver;

#[async_trait]
impl WorkerClassResolver for DefaultWorkerClassResolver {
    async fn resolve(
        &self,
        _conn: &sea_orm::DatabaseConnection,
        _workspace_id: Uuid,
    ) -> Result<Option<WorkerClass>, YorishiroError> {
        Ok(None)
    }
}

/// The resolver a deployment gets when it does not choose one.
#[must_use]
pub fn default_worker_class_resolver() -> Arc<dyn WorkerClassResolver> {
    Arc::new(DefaultWorkerClassResolver)
}

/// Only `workspace_id`/`entity_id`, not the `EntityRecord` itself: the record is re-read from the database inside `perform`, so a create-then-update (or create-then-delete) racing ahead of a still-queued job is picked up as the entity's current state rather than overwriting it with whatever was true at enqueue time.
/// A deleted entity is simply not found when re-read, and the job is a no-op for it.
///
/// No `model` field: `perform` re-resolves the provider from `workspace_id` on every run, and
/// `EmbeddingProvider` exposes no model identifier to mirror here anyway. Carrying one would be a
/// value nothing reads and nothing keeps in sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingSyncArgs {
    pub workspace_id: Uuid,
    pub entity_id: Uuid,
    pub worker_class: WorkerClass,
}

/// Loco has no automatic retry for any queue driver (see `Queue::retry_failed`'s own doc comment): `Ok` marks the job `Completed` in `pg_loco_queue` and `Err` marks it `Failed`, and only `Failed` is something an operator can find and re-run with `retry_failed`.
/// A structural failure (a schema no longer defining the field being embedded, a workspace's stamped dimension count not matching what the provider produces) will not go away on retry, so it is logged and reported `Ok`: retrying it would just mark the same job `Completed` again with nothing to show for the failure.
/// A transient one (`ProviderBusy`, `ProviderUnreachable`, or `Internal`) propagates as `Err`, which
/// is what `Failed` plus `retry_failed` exist for. Reporting them `Ok` would let a whole provider
/// outage mark itself `Completed` with every embedding still `NULL` and nothing recording that
/// anything went wrong — indistinguishable from jobs that never needed to run.
///
/// Shared by all three `WorkerClass` worker types below: the only difference between `EmbeddingSyncWorkerTenantPrivate`/`Official`/`Shared` is which tag `tags()` returns, so this function is the single place the actual sync logic lives rather than being copy-pasted three times.
async fn perform_embedding_sync(ctx: &AppContext, args: &EmbeddingSyncArgs) -> loco_rs::Result<()> {
    let provider = match crate::controllers::extractors::resolve_embedding_provider(
        ctx,
        args.workspace_id,
    )
    .await
    {
        Ok(provider) => provider,
        Err(err) => {
            tracing::warn!(entity_id = %args.entity_id, error = %err.0, "embedding sync worker: no embedding provider configured");
            return Ok(());
        }
    };

    let record = match content_entities::get(&ctx.db, args.workspace_id, args.entity_id).await {
        Ok(record) => record,
        Err(crate::error::YorishiroError::NotFound { .. }) => {
            tracing::debug!(entity_id = %args.entity_id, "embedding sync worker: entity no longer exists, skipping");
            return Ok(());
        }
        Err(err) => {
            tracing::warn!(entity_id = %args.entity_id, error = %err, "embedding sync worker: failed to re-read entity");
            return Err(err.into());
        }
    };

    if let Err(err) = embedding::sync::sync_embedding_for_record(
        &ctx.db,
        args.workspace_id,
        &record,
        provider.as_ref(),
    )
    .await
    {
        use crate::error::YorishiroError;
        match err {
            YorishiroError::ProviderBusy { .. }
            | YorishiroError::ProviderUnreachable { .. }
            | YorishiroError::Internal(_) => {
                tracing::warn!(entity_id = %args.entity_id, error = %err, "embedding sync failed transiently, job will be marked failed for retry_failed");
                return Err(err.into());
            }
            YorishiroError::ValidationFailed { .. } | YorishiroError::NotFound { .. } => {
                tracing::warn!(entity_id = %args.entity_id, error = %err, "embedding sync failed structurally, will not be retried");
            }
            other => {
                // Every other variant reaches this path only via an unexpected future change to sync_embedding_for_record's error surface; treat as structural (not retried) rather than silently falling through, and the log line makes an unclassified variant visible instead of quietly swallowed.
                tracing::warn!(entity_id = %args.entity_id, error = %other, "embedding sync failed with an unclassified error, treating as non-retryable");
            }
        }
    }

    Ok(())
}

/// Declares one `WorkerClass`'s worker type: a thin struct whose only job is to give `tags()` a fixed single tag, so that class's jobs are visible only to a worker process that asked for that tag.
/// `perform` on every generated type delegates to [`perform_embedding_sync`] unchanged; the three types differ in nothing but `tags()`'s return value.
macro_rules! embedding_sync_worker_for_class {
    ($worker_ty:ident, $class:expr) => {
        #[doc = concat!("`EmbeddingSyncWorker` restricted to `", stringify!($class), "` jobs: registered under its own `class_name()`, so `enqueue_for_class` enqueues under this type's name for that `WorkerClass` and a worker process started with `--worker=", stringify!($class), "` dequeues only jobs this type enqueued.")]
        pub struct $worker_ty {
            ctx: AppContext,
        }

        #[async_trait]
        impl BackgroundWorker<EmbeddingSyncArgs> for $worker_ty {
            fn build(ctx: &AppContext) -> Self {
                Self { ctx: ctx.clone() }
            }

            fn tags() -> Vec<String> {
                vec![$class.tag().to_string()]
            }

            async fn perform(&self, args: EmbeddingSyncArgs) -> loco_rs::Result<()> {
                perform_embedding_sync(&self.ctx, &args).await
            }
        }
    };
}

embedding_sync_worker_for_class!(EmbeddingSyncWorkerTenantPrivate, WorkerClass::TenantPrivate);
embedding_sync_worker_for_class!(EmbeddingSyncWorkerOfficial, WorkerClass::Official);
embedding_sync_worker_for_class!(EmbeddingSyncWorkerShared, WorkerClass::Shared);

/// Enqueues `args` on the worker type matching `args.worker_class`, so the tag `pg_loco_queue.tags` actually carries is the one the caller resolved, not every class's tag at once.
///
/// Exhaustively matched on `WorkerClass` (no `_` arm) on purpose: adding a fourth `WorkerClass` variant without also adding its worker type here fails to compile, rather than silently falling through to the wrong queue the way a wildcard arm would let it.
pub async fn enqueue_for_class(ctx: &AppContext, args: EmbeddingSyncArgs) -> loco_rs::Result<()> {
    let result = match args.worker_class {
        WorkerClass::TenantPrivate => EmbeddingSyncWorkerTenantPrivate::perform_later(ctx, args),
        WorkerClass::Official => EmbeddingSyncWorkerOfficial::perform_later(ctx, args),
        WorkerClass::Shared => EmbeddingSyncWorkerShared::perform_later(ctx, args),
    }
    .await;
    result.map(|_job_id| ())
}

/// Enqueues embedding sync after the caller's own transaction has committed: generating a vector is an HTTP round trip to the embedding provider (up to 30s), and this must never add that latency to the entity write it follows, nor hold a DB connection open for it.
/// `perform_later` in `BackgroundQueue` mode only inserts a row into `pg_loco_queue` and returns; the embedding provider round trip happens later, inside whichever `WorkerClass` worker type's `perform` dequeues the job (see [`enqueue_for_class`]), on a worker process, not on this request's task.
/// Runs on Loco's own `BackgroundQueue` (`pg_loco_queue`), so a process restart, a forced kill, or a provider outage that exhausts its own retries does not silently lose the sync: the job survives in the queue table for the next worker run.
/// A failure to enqueue at all (queue provider unreachable) is only logged: the entity write already succeeded and embedding is an auxiliary feature, so no failure here should surface to the caller.
///
/// This lives here rather than beside one transport's handlers because both of them need it: every entity write that does not call this leaves `content_entities.embedding` NULL forever, and such an entity is reachable only through the `pg_trgm` fuzzy fallback, so the symptom is search quietly returning worse results rather than any error.
pub(crate) async fn enqueue_after_write(ctx: &AppContext, workspace_id: Uuid, entity_id: Uuid) {
    let worker_class = match crate::controllers::extractors::resolve_worker_class(ctx, workspace_id)
        .await
    {
        Ok(worker_class) => worker_class,
        Err(err) => {
            tracing::warn!(entity_id = %entity_id, error = %err.0, "failed to resolve worker class, defaulting to shared");
            WorkerClass::Shared
        }
    };
    let args = EmbeddingSyncArgs {
        workspace_id,
        entity_id,
        worker_class,
    };
    if let Err(err) = enqueue_for_class(ctx, args).await {
        tracing::warn!(entity_id = %entity_id, error = %err, "failed to enqueue embedding sync");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each of the three worker types must carry exactly its own `WorkerClass`'s tag and no other: a type whose `tags()` drifted to list a second class's tag (or dropped its own) would let a tag-restricted worker process either miss its own jobs or pick up another class's, exactly the bug this whole split exists to close.
    #[test]
    fn each_worker_type_carries_exactly_its_own_class_tag() {
        assert_eq!(
            EmbeddingSyncWorkerTenantPrivate::tags(),
            vec![WorkerClass::TenantPrivate.tag().to_string()]
        );
        assert_eq!(
            EmbeddingSyncWorkerOfficial::tags(),
            vec![WorkerClass::Official.tag().to_string()]
        );
        assert_eq!(
            EmbeddingSyncWorkerShared::tags(),
            vec![WorkerClass::Shared.tag().to_string()]
        );
    }

    /// `serde(rename_all = "snake_case")` is what `EmbeddingSyncArgs` actually persists into `pg_loco_queue`'s `task_data`; asserting the wire form catches an accidental rename breaking a job already sitting in the queue at deploy time.
    #[test]
    fn worker_class_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_value(WorkerClass::TenantPrivate).unwrap(),
            serde_json::json!("tenant_private")
        );
        assert_eq!(
            serde_json::to_value(WorkerClass::Official).unwrap(),
            serde_json::json!("official")
        );
        assert_eq!(
            serde_json::to_value(WorkerClass::Shared).unwrap(),
            serde_json::json!("shared")
        );
    }

    /// `as_db_str`/`from_db_str` must round-trip every variant, and must agree with the `snake_case` serde wire form above: `ee/`'s `identity_workspace_worker_classes` stores this same string, so a row read from the database and a value read off a queued job's payload must be indistinguishable.
    #[test]
    fn db_str_round_trips_and_matches_the_serde_wire_form() {
        for class in [
            WorkerClass::TenantPrivate,
            WorkerClass::Official,
            WorkerClass::Shared,
        ] {
            let db_str = class.as_db_str();
            assert_eq!(
                WorkerClass::from_db_str(db_str).unwrap(),
                class,
                "as_db_str/from_db_str must round-trip {class:?}"
            );
            assert_eq!(
                serde_json::to_value(class).unwrap(),
                serde_json::json!(db_str),
                "{class:?}'s db string must match its serde wire form"
            );
        }
    }

    #[test]
    fn from_db_str_rejects_an_unknown_value() {
        assert!(WorkerClass::from_db_str("not-a-real-class").is_err());
    }

    /// `App::connect_workers` registers each of the three types under its own `class_name()`.
    /// Registering needs a real `Queue`, which a unit test has no access to, so this guards the
    /// assumption instead: three distinct class names, or one `register` call silently clobbers
    /// another's handler rather than adding a third.
    /// `enqueue_for_class`'s exhaustive `match` on `WorkerClass` already forces a compile error if a fourth variant is added with no worker type to dispatch to; this test covers the complementary runtime gap `connect_workers` itself has no compiler check for: a worker type that exists and is dispatched to, but was never actually registered.
    #[test]
    fn the_three_worker_types_have_distinct_class_names() {
        let names = [
            EmbeddingSyncWorkerTenantPrivate::class_name(),
            EmbeddingSyncWorkerOfficial::class_name(),
            EmbeddingSyncWorkerShared::class_name(),
        ];
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "worker types must have distinct class_name()s, got {names:?}"
        );
    }
}
