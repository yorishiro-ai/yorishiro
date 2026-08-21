use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::Semaphore;
use tokio_util::task::TaskTracker;
use uuid::Uuid;
use yorishiro_core::db::{DbHandle, TenantDb};
use yorishiro_core::models::entities::EntityRecord;
use yorishiro_core::services::auth::{Authenticator, default_authenticator};
use yorishiro_core::services::embedding::EmbeddingProvider;
use yorishiro_core::services::embedding::sync as embedding_sync;
use yorishiro_core::services::queue::{LocalQueue, Queue};
use yorishiro_core::{ResultExt, YorishiroError};

/// Cap on concurrent background embedding syncs.
/// Each sync task holds a pool connection for the duration of the embedding API call (up to tens of seconds), so spawning without limit would exhaust the connections needed for request handling (20 total in the pool) during a write burst.
/// Tasks beyond the cap aren't dropped: they wait on the semaphore without holding a connection.
const EMBEDDING_SYNC_MAX_CONCURRENCY: usize = 4;

/// How many times a busy provider is waited on before the task gives up and leaves the entity to `admin resync-embeddings`.
/// Bounded so a provider in a long outage cannot accumulate tasks that each sleep indefinitely.
const EMBEDDING_SYNC_MAX_RETRIES: u32 = 3;

/// Application state shared by both the REST and MCP handlers.
/// Using this struct as axum's `State` (rather than `TenantDb` alone) lets search handlers also reach the `EmbeddingProvider`.
#[derive(Clone)]
pub struct AppState {
    pub db: DbHandle,
    pub embedding_provider: Arc<dyn EmbeddingProvider>,
    /// Per-workspace token budget for search.
    /// Search is metered in tokens rather than requests because that is what it costs the embedding model, and because a query is short enough that counting it is cheap: measured at 74µs, against 165ms for a large entity body, which is why writes stay on request counts.
    pub search_token_limiter: Arc<crate::http::middleware::rate_limit::RateLimiter>,
    /// How a presented API key becomes an `AuthContext`.
    /// Every REST extractor and every MCP handler resolves through this one value, so replacing it changes authentication for the whole process rather than for the paths that remembered to ask.
    ///
    /// Defaults to `DefaultAuthenticator` (this crate's own rule) via [`AppState::new`].
    /// Use [`AppState::with_authenticator`] to supply another.
    pub authenticator: Arc<dyn Authenticator>,
    embedding_sync_permits: Arc<Semaphore>,
    embedding_tasks: TaskTracker,
    /// Where deferred work runs.
    /// Held as the trait so a deployment that needs tasks to survive the process can supply a driver that outlives it, without every caller of `spawn_embedding_sync` learning about queues.
    queue: Arc<dyn Queue>,
}

impl AppState {
    /// Builds state for a Postgres deployment.
    /// Kept as the two-pool shape every existing caller already passes; [`AppState::from_handle`] is the Sqlite-capable entry point underneath.
    pub fn new(
        tenant_db: TenantDb,
        identity_pool: PgPool,
        embedding_provider: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self::from_handle(
            DbHandle::Postgres {
                tenant: tenant_db,
                identity: identity_pool,
            },
            embedding_provider,
        )
    }

    pub fn from_handle(db: DbHandle, embedding_provider: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            db,
            embedding_provider,
            // Built here rather than passed in, so every existing caller of `new` keeps working and a downstream process gets the same quota without asking for it.
            search_token_limiter: Arc::new(
                crate::http::middleware::rate_limit::RateLimiter::search_tokens_from_env(),
            ),
            authenticator: default_authenticator(),
            embedding_sync_permits: Arc::new(Semaphore::new(EMBEDDING_SYNC_MAX_CONCURRENCY)),
            embedding_tasks: TaskTracker::new(),
            queue: Arc::new(LocalQueue::new(EMBEDDING_SYNC_MAX_CONCURRENCY)),
        }
    }

    /// The control-plane pool, for the handful of endpoints (signup, login, invite redemption, setup) that must read/write `identity.users`/`identity.tenant_memberships`/`identity.invites` before any tenant or workspace context can be established.
    /// Postgres connects it with the migration role (not `yorishiro_app`), bypassing RLS entirely; every other handler must keep using [`AppState::tenant_db`] instead.
    /// Sqlite has no role to separate and no RLS to bypass, so nothing here can hand back a second pool: callers on that engine need `setup`'s own match on `db` rather than this method.
    pub fn identity_pool(&self) -> Result<&PgPool, YorishiroError> {
        match &self.db {
            DbHandle::Postgres { identity, .. } => Ok(identity),
            DbHandle::Sqlite(_) => Err(YorishiroError::Internal(anyhow::anyhow!(
                "identity_pool is not available on the Sqlite engine"
            ))),
        }
    }

    /// The Postgres tenant pool, for handlers not yet generalized over [`yorishiro_core::db::Storage`].
    /// Fails loudly on Sqlite rather than silently narrowing behavior, the same posture [`yorishiro_core::services::auth::authorize`] takes.
    pub fn tenant_db(&self) -> Result<&TenantDb, YorishiroError> {
        match &self.db {
            DbHandle::Postgres { tenant, .. } => Ok(tenant),
            DbHandle::Sqlite(_) => Err(YorishiroError::Internal(anyhow::anyhow!(
                "this operation is not available on the Sqlite engine yet"
            ))),
        }
    }

    /// Charges a search query against its workspace's token budget, refusing when the budget is spent.
    ///
    /// Charged before embedding, since embedding is the work the budget protects, and counting is cheap (a query is short), which is why search is metered in tokens while writes stay on request counts.
    /// Lives here rather than in either adapter because both reach the same provider: a check written in only one of them leaves the other able to spend the budget it is meant to protect.
    pub fn charge_search_tokens(
        &self,
        workspace_id: uuid::Uuid,
        query_text: &str,
    ) -> Result<(), YorishiroError> {
        let tokens = self.embedding_provider.count_tokens(query_text);
        if self
            .search_token_limiter
            .allow_cost(&workspace_id.to_string(), tokens)
        {
            return Ok(());
        }

        tracing::warn!(%workspace_id, tokens, "search token budget exhausted");
        Err(YorishiroError::ValidationFailed {
            message: "this workspace has spent its search token budget for the minute".to_string(),
            details: vec![],
            hint: "retry shortly, or raise YORISHIRO_SEARCH_TOKENS_PER_MINUTE".to_string(),
        })
    }

    /// Replaces how this process authenticates.
    /// See [`Authenticator`] for the contract an implementation must hold to.
    pub fn with_authenticator(mut self, authenticator: Arc<dyn Authenticator>) -> Self {
        self.authenticator = authenticator;
        self
    }

    /// Tracker used to wait for in-flight embedding syncs during graceful shutdown.
    /// `main` calls `close()` + `wait()` on it after the HTTP server stops.
    pub fn embedding_tasks(&self) -> &TaskTracker {
        &self.embedding_tasks
    }

    /// Hands deferred work to this process's queue.
    ///
    /// Where `spawn_embedding_sync` is the one deferred job this crate has and returns a handle its callers await in tests, this is the general seam: a deployment that needs work to survive the process supplies a driver that outlives it, and nothing at the call site changes.
    /// The embedding sync keeps its own path until there is a second driver to justify moving it: a refactor with no second implementation to prove it is a guess about what the second one will need.
    pub fn enqueue(&self, task: yorishiro_core::services::queue::Task) {
        self.queue.enqueue(task);
    }

    /// Waits for queued work at shutdown.
    /// See [`Queue::drain`].
    pub async fn drain_queue(&self, timeout: std::time::Duration) {
        self.queue.drain(timeout).await;
    }

    /// Syncs the `embedding` column in the background after an entity create/update succeeds.
    /// The embedding API call can take up to tens of seconds, so the request isn't made to wait for it, and a fresh connection is acquired from the pool instead of reusing the request's own connection (satisfying the no-same-transaction constraint documented on `sync_embedding`).
    /// Failures are only logged: embedding is an auxiliary feature and must not affect whether the entity write itself succeeds.
    ///
    /// Postgres only: search embedding storage is `pgvector`, which Sqlite has no equivalent of, so a caller on that engine gets a task that logs and returns rather than one that panics reaching for a pool that isn't there.
    pub fn spawn_embedding_sync(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
        record: EntityRecord,
    ) -> tokio::task::JoinHandle<()> {
        let db = self.tenant_db().cloned();
        let provider = Arc::clone(&self.embedding_provider);
        let permits = Arc::clone(&self.embedding_sync_permits);
        // Spawning through the TaskTracker lets graceful shutdown wait for the embedding sync of an already-written entity to finish (an immediate SIGTERM exit would lose the sync, leaving that entity permanently missing from search).
        self.embedding_tasks.spawn(async move {
            let Ok(db) = db else {
                tracing::warn!(entity_id = %record.id, "embedding sync skipped: no pgvector storage on this engine");
                return;
            };

            // The order matters: acquire the permit before the connection.
            // Reversing it would let every waiting task hold a connection, defeating the point of the cap.
            let Ok(_permit) = permits.acquire_owned().await else {
                // Unreachable in practice: the semaphore is never closed.
                return;
            };

            // A provider asking to be tried again is worth waiting for: the alternative is losing this entity from search until someone runs a resync.
            // Bounded attempts, because a provider that stays busy should not hold a task forever.
            // The resync path is what covers the case where it does.
            let mut attempt = 0;
            let result = loop {
                let outcome = async {
                    let mut conn = db
                        .acquire_for_workspace(tenant_id, workspace_id)
                        .await
                        .internal()?;
                    embedding_sync::sync_embedding_for_record(
                        &mut conn,
                        workspace_id,
                        &record,
                        provider.as_ref(),
                    )
                    .await
                }
                .await;

                match outcome {
                    Err(YorishiroError::ProviderBusy {
                        ref message,
                        retry_after,
                    }) if attempt < EMBEDDING_SYNC_MAX_RETRIES => {
                        attempt += 1;
                        tracing::info!(
                            entity_id = %record.id,
                            attempt,
                            retry_after_secs = retry_after.as_secs(),
                            %message,
                            "embedding provider busy; waiting before retry"
                        );
                        // The connection is released before sleeping: it was dropped with the block above.
                        // Holding one through the wait would spend the pool on tasks that are doing nothing.
                        tokio::time::sleep(retry_after).await;
                    }
                    other => break other,
                }
            };

            if let Err(err) = result {
                tracing::warn!(entity_id = %record.id, error = %err, "embedding sync failed");
            }
        })
    }
}

#[cfg(test)]
#[path = "../tests/state.rs"]
mod tests;
