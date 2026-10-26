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

use super::boot_request_isolated;

/// Configures and registers a shared-target consumer through Loco, without starting its loop.
///
/// A Redis worker polls only the queues its configuration names, which `yorishiro start --worker=...` fills in from the targets it serves.
/// Updates the worker context before building its queue and running Loco's worker boot hooks over the already migrated database.
pub(crate) async fn shared_target_queue(ctx: &mut AppContext) -> Arc<loco_rs::bgworker::Queue> {
    target_queue(ctx, WorkerTarget::Shared).await
}

pub(crate) async fn target_queue(
    ctx: &mut AppContext,
    target: WorkerTarget,
) -> Arc<loco_rs::bgworker::Queue> {
    queue_for_tag(ctx, target.queue()).await
}

#[cfg(feature = "enterprise")]
pub(crate) async fn job_target_queue(
    ctx: &mut AppContext,
    target: WorkerTarget,
) -> Arc<loco_rs::bgworker::Queue> {
    queue_for_tag(ctx, target.job_tag()).await
}

pub(crate) async fn queue_for_tag(
    ctx: &mut AppContext,
    tag: String,
) -> Arc<loco_rs::bgworker::Queue> {
    let mode = loco_rs::boot::StartMode::WorkerOnly { tags: vec![tag] };
    yorishiro::workers::start::configure(&mut ctx.config, &mode)
        .expect("configure production worker role");
    let queue = loco_rs::bgworker::create_queue_provider(&ctx.config)
        .await
        .expect("create worker queue")
        .expect("background queue mode");
    queue.setup().await.expect("set up worker queue");
    ctx.queue_provider = Some(queue.clone());
    let boot = loco_rs::boot::run_app::<App>(&mode, ctx.clone())
        .await
        .expect("boot production worker through Loco");
    *ctx = boot.app_context;
    queue
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
    boot_with_query_worker_and_drain(provider, callback, |_| async {}).await;
}

#[allow(clippy::future_not_send)]
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
    boot_request_isolated::<App, _, _>(run).await;
}
