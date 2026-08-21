use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use sqlx::PgPool;
use yorishiro_core::error::YorishiroError;
use yorishiro_core::services::embedding::EmbeddingProvider;

use super::*;

/// Counts how many calls are in flight at once and remembers the high-water mark, so a test can assert the semaphore actually bounded them rather than assert the constant it was built from.
struct ConcurrencyProbe {
    in_flight: AtomicUsize,
    peak: AtomicUsize,
}

impl ConcurrencyProbe {
    fn new() -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for ConcurrencyProbe {
    fn dimensions(&self) -> usize {
        3
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);

        // Long enough that every spawned task piles up here if the cap is not enforced.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(texts.iter().map(|_| vec![0.0, 0.0, 0.0]).collect())
    }
}

/// Never returns, so a task holding a permit keeps holding it.
/// Used to prove the permit is taken before a connection, not after.
struct HangingProvider;

#[async_trait]
impl EmbeddingProvider for HangingProvider {
    fn dimensions(&self) -> usize {
        3
    }

    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        std::future::pending().await
    }
}

/// Seeds a workspace whose schema has an `x-embed` field, and returns the ids plus a record shaped for it.
/// Without a schema declaring `x-embed`, `sync_embedding_for_record` returns early and the provider is never called, so these tests would pass while proving nothing.
async fn seed_embeddable(
    pool: &PgPool,
) -> (
    uuid::Uuid,
    uuid::Uuid,
    yorishiro_core::models::entities::EntityRecord,
) {
    let (tenant_id, workspace_id) = crate::test_support::seed_workspace(pool).await;

    let definition = serde_json::from_value(serde_json::json!({
        "name": "notes",
        "entity_types": {
            "note": { "fields": { "body": { "type": "string", "x-embed": true } } }
        }
    }))
    .unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let (schema, _) = yorishiro_core::models::schemas::create_schema(
        &mut conn,
        tenant_id,
        workspace_id,
        definition,
    )
    .await
    .unwrap();
    drop(conn);

    let record = yorishiro_core::models::entities::EntityRecord {
        id: uuid::Uuid::new_v4(),
        workspace_id,
        schema_id: schema.id,
        schema_version: schema.version,
        entity_type: "note".into(),
        data: serde_json::json!({ "body": "text" }),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        created_by: None,
        updated_by: None,
    };

    (tenant_id, workspace_id, record)
}

/// The semaphore is what stops a burst of entity writes from opening an unbounded number of provider calls.
/// Asserting the constant's value proves nothing: this spawns more syncs than the cap allows and checks that no more than the cap ever ran at once.
#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_embedding_syncs_never_exceed_the_cap(pool: PgPool) {
    let (tenant_id, workspace_id, record) = seed_embeddable(&pool).await;

    let probe = Arc::new(ConcurrencyProbe::new());
    let state = AppState::new(
        yorishiro_core::db::TenantDb::new(pool.clone()),
        pool,
        Arc::clone(&probe) as Arc<dyn EmbeddingProvider>,
    );

    let spawned: Vec<_> = (0..EMBEDDING_SYNC_MAX_CONCURRENCY * 3)
        .map(|_| state.spawn_embedding_sync(tenant_id, workspace_id, record.clone()))
        .collect();

    for handle in spawned {
        handle.await.unwrap();
    }

    let peak = probe.peak.load(Ordering::SeqCst);
    assert!(peak > 0, "no sync ever reached the provider");
    assert!(
        peak <= EMBEDDING_SYNC_MAX_CONCURRENCY,
        "{peak} syncs ran at once against a cap of {EMBEDDING_SYNC_MAX_CONCURRENCY}"
    );
}

/// `spawn_embedding_sync` acquires the permit *before* the connection, deliberately: reversing the two would let every waiting task hold a pool connection while queued, which is what the cap exists to prevent.
/// With a provider that never returns, the tasks past the cap must be parked on the semaphore holding nothing, so the pool still hands out connections.
#[sqlx::test(migrations = "../../migrations")]
async fn queued_syncs_do_not_hold_a_connection_while_waiting(pool: PgPool) {
    let (tenant_id, workspace_id, record) = seed_embeddable(&pool).await;

    let state = AppState::new(
        yorishiro_core::db::TenantDb::new(pool.clone()),
        pool.clone(),
        Arc::new(HangingProvider),
    );

    // Far more than the cap; every one past it should be waiting on a permit, not a connection.
    for _ in 0..EMBEDDING_SYNC_MAX_CONCURRENCY * 4 {
        state.spawn_embedding_sync(tenant_id, workspace_id, record.clone());
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // If queued tasks held connections, this would block until the pool timed out.
    let acquired = tokio::time::timeout(std::time::Duration::from_secs(5), pool.acquire()).await;

    assert!(
        acquired.is_ok_and(|r| r.is_ok()),
        "queued embedding syncs were holding pool connections while waiting for a permit"
    );
}

/// Syncs are spawned through the `TaskTracker` so graceful shutdown can wait for an already written entity's embedding to land: an immediate exit would leave that entity permanently missing from search.
/// A sync spawned outside the tracker would not be waited for.
#[sqlx::test(migrations = "../../migrations")]
async fn shutdown_waits_for_in_flight_syncs(pool: PgPool) {
    let (tenant_id, workspace_id, record) = seed_embeddable(&pool).await;

    let probe = Arc::new(ConcurrencyProbe::new());
    let state = AppState::new(
        yorishiro_core::db::TenantDb::new(pool.clone()),
        pool,
        Arc::clone(&probe) as Arc<dyn EmbeddingProvider>,
    );

    state.spawn_embedding_sync(tenant_id, workspace_id, record.clone());

    // What `main` does on SIGTERM.
    state.embedding_tasks().close();
    state.embedding_tasks().wait().await;

    assert_eq!(
        probe.in_flight.load(Ordering::SeqCst),
        0,
        "wait() returned while a sync was still running"
    );
    assert!(
        probe.peak.load(Ordering::SeqCst) > 0,
        "the sync never ran, so waiting for it proved nothing"
    );
}

/// The queue seam is reachable from AppState and drains what it accepted.
/// Without this the trait would exist while nothing in the process could hand work to it.
#[sqlx::test(migrations = "../../migrations")]
async fn app_state_runs_and_drains_queued_work(pool: PgPool) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let probe = Arc::new(ConcurrencyProbe::new());
    let state = AppState::new(
        yorishiro_core::db::TenantDb::new(pool.clone()),
        pool,
        Arc::clone(&probe) as Arc<dyn EmbeddingProvider>,
    );
    let ran = Arc::new(AtomicUsize::new(0));

    for _ in 0..3 {
        let ran = Arc::clone(&ran);
        state.enqueue(Box::pin(async move {
            ran.fetch_add(1, Ordering::SeqCst);
        }));
    }

    state.drain_queue(std::time::Duration::from_secs(5)).await;
    assert_eq!(ran.load(Ordering::SeqCst), 3);
}

/// The search token budget is charged per workspace and refuses once spent.
///
/// Asserted on `AppState` because that is the seam both adapters go through.
/// The MCP tool did the same embedding work with no charge at all until this moved out of the REST
/// handler, so a test entering only through `GET /api/search` would have passed the whole time.
#[sqlx::test(migrations = "../../migrations")]
async fn search_tokens_are_charged_per_workspace_and_run_out(pool: PgPool) {
    use yorishiro_core::models::tenancy;

    // Real workspaces, so the ids are the `DEFAULT uuidv7()` ones the database issues.
    // Nothing in this process mints an id, so a test that minted its own would be keying the
    // limiter on a shape production never sees.
    let tenant = tenancy::create_tenant(&pool, "budget", None).await.unwrap();
    let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None, None, None)
        .await
        .unwrap()
        .id;
    let other = tenancy::create_workspace(&pool, tenant.id, "second", None, None, None)
        .await
        .unwrap()
        .id;

    let mut state = AppState::new(
        yorishiro_core::db::TenantDb::new(pool.clone()),
        pool,
        Arc::new(ConcurrencyProbe::new()) as Arc<dyn EmbeddingProvider>,
    );
    // Set directly rather than through `YORISHIRO_SEARCH_TOKENS_PER_MINUTE`, which is process-wide
    // and would race the other tests in this binary.
    state.search_token_limiter = Arc::new(crate::http::middleware::rate_limit::RateLimiter::new(
        8,
        std::time::Duration::from_secs(60),
    ));

    // The default `count_tokens` is one token per four characters, so this is five.
    let query = "a".repeat(20);

    state.charge_search_tokens(workspace, &query).unwrap();
    assert!(
        state.charge_search_tokens(workspace, &query).is_err(),
        "a second five-token query should exceed an eight-token budget"
    );

    // The budget is per workspace, so one spending it does not silence another.
    state.charge_search_tokens(other, &query).unwrap();
}
