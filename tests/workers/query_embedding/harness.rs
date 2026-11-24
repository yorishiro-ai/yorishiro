//! A booted app with a real queue worker, for tests that need the server to ask a worker to embed a search query.
//!
//! The server under test holds no embedding model, so every semantic search needs a worker serving the shared target, where `query-embedding` jobs go.
//! The worker runs against the booted app's own queue provider, the one the HTTP handlers enqueue to.
//! On SQLite the worker uses the same queue file that request boot creates for the test.
//! Callers that override query embedding settings still take `serial(process_environment)`.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use loco_rs::TestServer;
use loco_rs::app::{AppContext, Hooks};
use yorishiro::App;
use yorishiro::services::embedding::EmbeddingProvider;
use yorishiro::workers::target::WorkerTarget;

use crate::requests::{boot_request, jobs::shared_target_queue};

/// Boots the app, installs `provider` as the worker's deployment default, and runs a query embedding worker against the app's queue until `callback` returns.
#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
pub(crate) async fn boot_with_query_worker<F, Fut>(
    provider: Arc<dyn EmbeddingProvider>,
    callback: F,
) where
    F: FnOnce(TestServer, AppContext) -> Fut,
    Fut: Future<Output = ()>,
{
    boot_with_query_worker_and_drain(provider, callback, |_| async {}).await;
}

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
pub(crate) async fn boot_with_query_worker_and_drain<F, Fut, D, Done>(
    provider: Arc<dyn EmbeddingProvider>,
    callback: F,
    after_drain: D,
) where
    F: FnOnce(TestServer, AppContext) -> Fut,
    Fut: Future<Output = ()>,
    D: FnOnce(AppContext) -> Done,
    Done: Future<Output = ()>,
{
    let run = move |request: TestServer, ctx: AppContext| async move {
        ctx.shared_store.insert(provider);
        let mut worker_ctx = ctx.clone();
        let queue = shared_target_queue(&mut worker_ctx).await;
        let running = queue.clone();
        let worker =
            tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });

        let outcome = std::panic::AssertUnwindSafe(callback(request, ctx.clone()))
            .catch_unwind()
            .await;

        App::on_shutdown(&worker_ctx).await;
        let shutdown_result = queue.shutdown();
        let worker_result = tokio::time::timeout(Duration::from_secs(10), worker).await;
        let close_result = queue.close().await;
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
        shutdown_result.expect("stop query worker");
        worker_result
            .expect("drain query worker")
            .expect("join query worker")
            .expect("run query worker");
        close_result.expect("close query worker queue");
        after_drain(ctx).await;
    };
    boot_request::<App, _, _>(run).await;
}
