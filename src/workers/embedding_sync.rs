//! Generates and stores an entity's embedding vector via Loco's own `BackgroundQueue`.
//!
//! Replaces the `tokio::spawn` this deployment used before a queue provider existed: that stand-in lost every in-flight sync on a process restart, a forced kill, or a provider outage past its own retry budget, leaving the entity's `embedding` column permanently `NULL` with nothing to retry it (see `tasks::resync_embeddings`'s doc comment for the operational recovery command that gap required).
//! `pg_loco_queue` persists the job in Postgres, so a re-deployed or restarted process resumes it instead of losing it.
//!
//! **There is no "subscribe to every `WorkerClass`" worker mode.** A worker started with no `--worker=<tags>` argument (bare `--worker`, or the worker half of `--server-and-worker`) does not dequeue every tagged job; confirmed against `loco-rs` 1.1.0's own dequeue SQL (`bgworker/pg.rs`'s `dequeue`, and the matching logic in `sqlt.rs`/`redis.rs`), an empty tag list makes the query `AND (tags IS NULL)`, so an untagged worker dequeues only *untagged* jobs.
//! Every job this module enqueues always carries exactly one tag (its resolved `WorkerClass`'s own tag, via [`enqueue_for_class`]), so it is never untagged, so a bare `--worker` process run against this deployment dequeues none of these jobs, ever, not "the leftover ones nothing else claimed."
//! A deployment that wants one process to cover every class must start it with every tag named explicitly: `cargo loco start --worker=worker-class:tenant-private,worker-class:official,worker-class:shared`.
//! There is no wildcard or "ignore tags" flag in `loco-rs` 1.1.0; a class added to [`WorkerClass`] in the future needs that class's tag added to every such command by hand, the same way it needs a fourth worker type added here (`enqueue_for_class`'s exhaustive match forces the latter to be noticed at compile time; the former has no equivalent enforcement and is an operational runbook concern, not a code one).

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
/// Routes jobs via `BackgroundWorker::tags()` (loco-rs's own dequeue-time tag filter, shared by the Postgres, `SQLite`, and Redis queue providers, unlike `queues: Option<Vec<String>>`, which only exists on the Redis config) rather than a named queue: this deployment's queue provider is Postgres (`pg_loco_queue`), and `queue: Option<String>` is silently discarded by that provider's `enqueue` (no column for it), so a named-queue split would require switching to Redis first.
///
/// `tags()` is a `BackgroundWorker` trait method with no access to a specific job's own args (confirmed against `loco-rs` 1.1.0's `perform_later_with_priority`, which calls `Self::tags()` before it ever sees `args`): one worker *type* can carry only one fixed tag set, not a tag set chosen per enqueued job.
/// A single `EmbeddingSyncWorker` type whose `tags()` returned every `WorkerClass`'s tag (the shape this enum had before this comment) therefore could not route by class at all: every job it enqueued carried all three tags regardless of which one `resolve_worker_class` picked, so a tag-restricted `--worker=worker-class:tenant-private` process would dequeue *every* class's jobs, not just its own.
/// [`EmbeddingSyncWorkerTenantPrivate`], [`EmbeddingSyncWorkerOfficial`], and [`EmbeddingSyncWorkerShared`] exist to give each `WorkerClass` its own worker *type*, each with a `tags()` fixed to that one class's tag, so the class chosen at enqueue time (via [`enqueue_for_class`]) is the class whose type actually gets registered with the queue and whose tag actually lands in `pg_loco_queue.tags`.
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
/// No `model` field: `perform` already re-resolves the embedding provider from `workspace_id` via `WorkspaceEmbeddingResolver` on every run (the same live-lookup reasoning as not re-reading the entity at enqueue time), and `EmbeddingProvider` exposes no model identifier for a payload field to even mirror.
/// Carrying one here would be a value nothing reads and nothing keeps in sync with the resolver's own answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingSyncArgs {
    pub workspace_id: Uuid,
    pub entity_id: Uuid,
    pub worker_class: WorkerClass,
}

/// Loco has no automatic retry for any queue driver (see `Queue::retry_failed`'s own doc comment): `Ok` marks the job `Completed` in `pg_loco_queue` and `Err` marks it `Failed`, and only `Failed` is something an operator can find and re-run with `retry_failed`.
/// A structural failure (a schema no longer defining the field being embedded, a workspace's stamped dimension count not matching what the provider produces) will not go away on retry, so it is logged and reported `Ok`: retrying it would just mark the same job `Completed` again with nothing to show for the failure.
/// A transient one (`ProviderBusy`, `ProviderUnreachable`, or `Internal`, chiefly a DB failure on the write/read this function itself does) is exactly what `Failed` plus `retry_failed` exists for, so those propagate as `Err`: reporting them `Ok` would let an entire provider outage's worth of jobs mark themselves `Completed` with the embedding still `NULL` and no record that anything went wrong, which is indistinguishable from a job that never needed to run at all.
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
/// Runs on Loco's own `BackgroundQueue` (`pg_loco_queue`), so a process restart, a forced kill, or a provider outage that exhausts its own retries no longer silently loses the sync: the job survives in the queue table for the next worker run.
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

    /// `App::connect_workers` registers each of these three types under its own `class_name()` (a `Queue::register` call per type); this test cannot call `connect_workers` itself (it needs a real `Queue`, not available to a plain unit test), but it guards the assumption that call relies on: the three types must resolve to three distinct class names, or one `queue.register` call would silently clobber another's handler instead of adding a third one.
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
