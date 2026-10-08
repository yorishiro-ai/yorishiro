//! A booted app with a real queue worker, for tests that need the server to ask a worker to embed a search query.
//!
//! The server under test holds no embedding model, so every semantic search needs a worker consuming `query-embedding` jobs.
//! The worker runs against the booted app's own queue provider, the one the HTTP handlers enqueue to.
//! On SQLite that provider is pointed at a private queue file for the test.
//! On PostgreSQL it is the shared queue table, so callers also take `serial(queue_postgres)`.
//! Callers take `serial(process_environment)`: the queue location and the query embedding settings are read from the environment at boot.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum_test::TestServer;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use yorishiro::App;
use yorishiro::error::YorishiroError;
use yorishiro::services::embedding::{EmbedKind, EmbeddingProvider};
use yorishiro::workers::query_embedding::QueryEmbeddingWorker;

use super::{boot_request, is_sqlite_queue};

fn isolated_redis_url(url: &str) -> String {
    let database = uuid::Uuid::now_v7().as_u128() % 16;
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let base = base
        .rsplit_once('/')
        .filter(|(_, suffix)| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
        .map_or(base, |(prefix, _)| prefix);
    if query.is_empty() {
        format!("{base}/{database}")
    } else {
        format!("{base}/{database}?{query}")
    }
}

/// The tag the query embedding worker consumes, as `worker-tags` prints it.
pub(crate) const QUERY_TAG: &str = "query-embedding";

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

/// Boots the app, installs `provider` as the worker's deployment default, and runs a query embedding worker against the app's queue until `callback` returns.
#[allow(clippy::future_not_send)]
pub(crate) async fn boot_with_query_worker<F, Fut>(
    provider: Arc<dyn EmbeddingProvider>,
    callback: F,
) where
    F: FnOnce(TestServer, AppContext) -> Fut,
    Fut: Future<Output = ()>,
{
    let queue_dir = tempfile::tempdir().expect("queue tempdir");
    let guard = crate::EnvGuard::capture(&["QUEUE_URL"]);
    if is_sqlite_queue() {
        guard.set(
            "QUEUE_URL",
            format!(
                "sqlite://{}?mode=rwc",
                queue_dir.path().join("queue.sqlite3").display()
            ),
        );
    } else if let Ok(url) = std::env::var("QUEUE_URL")
        && url.starts_with("redis")
    {
        // Query workers use a fixed queue name; isolate Redis-backed tests by DB number.
        // A PostgreSQL queue is the shared queue table, which callers serialize instead.
        guard.set("QUEUE_URL", isolated_redis_url(&url));
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        ctx.shared_store.insert(provider);
        let queue = ctx
            .queue_provider
            .clone()
            .expect("the app's queue provider");
        queue
            .register(QueryEmbeddingWorker::build(&ctx))
            .await
            .expect("register the query embedding worker");
        let running = queue.clone();
        let worker = tokio::spawn(async move { running.run(vec![QUERY_TAG.to_owned()]).await });

        callback(request, ctx).await;

        let _ = queue.shutdown();
        let _ = worker.await;
    })
    .await;
    drop(guard);
}
