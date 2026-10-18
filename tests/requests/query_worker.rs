//! A booted app with a real queue worker, for tests that need the server to ask a worker to embed a search query.
//!
//! The server under test holds no embedding model, so every semantic search needs a worker serving the shared target, where `query-embedding` jobs go.
//! The worker runs against the booted app's own queue provider, the one the HTTP handlers enqueue to.
//! On SQLite the worker uses the same queue file that request boot creates for the test.
//! PostgreSQL and Valkey queue tests share `serial(queue_backend)`.
//! Callers take `serial(process_environment)`: the queue location and the query embedding settings are read from the environment at boot.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum_test::TestServer;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::async_trait;
use yorishiro::App;
use yorishiro::error::YorishiroError;
use yorishiro::services::embedding::{EmbedKind, EmbeddingProvider};
use yorishiro::workers::query_embedding::QueryEmbeddingWorker;
use yorishiro::workers::target::WorkerTarget;

use super::boot_request;

/// Embeds text as a one-hot vector whose axis comes from a keyword, so a query and a document that share a keyword are at distance zero and every other pair is orthogonal.
/// Records the kind of every call and can be made slow, to model a worker that answers after the search gave up.
pub(crate) struct KeywordProvider {
    pub(crate) kinds: Arc<Mutex<Vec<EmbedKind>>>,
    pub(crate) width: usize,
    pub(crate) delay: Duration,
}

impl KeywordProvider {
    pub(crate) fn new(width: usize) -> Self {
        Self {
            kinds: Arc::new(Mutex::new(Vec::new())),
            width,
            delay: Duration::ZERO,
        }
    }

    fn vector(&self, text: &str) -> Vec<f32> {
        let axis = ["alpha", "beta", "gamma"]
            .iter()
            .position(|word| text.contains(word))
            .unwrap_or(3);
        let mut vector = vec![0.0; self.width];
        vector[axis] = 1.0;
        vector
    }
}

#[async_trait]
impl EmbeddingProvider for KeywordProvider {
    fn dimensions(&self) -> usize {
        self.width
    }

    fn model_name(&self) -> String {
        "keyword-test-provider".into()
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Ok(texts.iter().map(|text| self.vector(text)).collect())
    }

    async fn embed_as(&self, kind: EmbedKind, text: &str) -> Result<Vec<f32>, YorishiroError> {
        self.kinds.lock().unwrap().push(kind);
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok(self.vector(text))
    }
}

/// Fails every embedding with the error `make` builds.
pub(crate) struct FailingProvider(pub(crate) fn() -> YorishiroError);

#[async_trait]
impl EmbeddingProvider for FailingProvider {
    fn dimensions(&self) -> usize {
        768
    }

    fn model_name(&self) -> String {
        "failing-test-provider".into()
    }

    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Err((self.0)())
    }
}

/// The queue a worker for the shared target runs on.
///
/// A Redis worker polls only the queues its configuration names, which `yorishiro start --worker=...` fills in from the targets it serves.
/// The booted test app is a server, so for Redis this builds the provider a worker process would have, over the same database.
async fn shared_target_queue(ctx: &AppContext) -> Arc<loco_rs::bgworker::Queue> {
    if let Some(loco_rs::config::QueueConfig::Redis(redis)) = ctx.config.queue.clone() {
        let mut redis = redis;
        redis.dangerously_flush = false;
        redis.queues = Some(vec![WorkerTarget::Shared.queue()]);
        let queue = Arc::new(
            loco_rs::bgworker::redis::create_provider(&redis)
                .await
                .expect("the worker's Redis queue provider"),
        );
        queue
            .setup()
            .await
            .expect("set up the worker's Redis queue");
        return queue;
    }
    ctx.queue_provider
        .clone()
        .expect("the app's queue provider")
}

/// Boots the app, installs `provider` as the worker's deployment default, and runs a query embedding worker against the app's queue until `callback` returns.
#[allow(clippy::future_not_send)]
pub(crate) async fn boot_with_query_worker<F, Fut>(
    provider: Arc<dyn EmbeddingProvider>,
    callback: F,
) where
    F: FnOnce(TestServer, AppContext) -> Fut,
    Fut: Future<Output = ()>,
{
    let guard = crate::EnvGuard::capture(&["QUEUE_URL"]);
    if let Ok(url) = std::env::var("QUEUE_URL")
        && url.starts_with("redis")
    {
        // Keep the reserved database unchanged; queue tests share a serialization lock.
        guard.set("QUEUE_URL", url);
    }
    let run = move |request: TestServer, ctx: AppContext| async move {
        ctx.shared_store.insert(provider);
        let queue = shared_target_queue(&ctx).await;
        queue
            .register(QueryEmbeddingWorker::build(&ctx))
            .await
            .expect("register the query embedding worker");
        let running = queue.clone();
        let worker =
            tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
        let relay_ctx = ctx.clone();
        let relay_queue = queue.clone();
        let relay = tokio::spawn(async move {
            let mut relay_ctx = relay_ctx;
            relay_ctx.queue_provider = Some(relay_queue);
            loop {
                yorishiro::workers::dispatch::recover_pending(&relay_ctx).await;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });

        callback(request, ctx).await;

        relay.abort();
        let _ = relay.await;
        let _ = queue.shutdown();
        let _ = worker.await;
    };
    boot_request::<App, _, _>(run).await;
    drop(guard);
}
