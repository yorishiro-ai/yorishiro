//! Queue provider, dispatcher and relay lifecycle wiring.

use std::sync::Arc;

use loco_rs::{Result, app::AppContext};

use crate::workers::dispatch::{EmbeddingSyncDispatcher, LocoJobDispatcher, ReindexDispatcher};

pub(crate) fn install(ctx: &AppContext) {
    ctx.shared_store
        .insert(crate::workers::embedding_sync::default_worker_class_resolver());
    ctx.shared_store
        .insert(crate::workers::queue::default_queue_policy());
    ctx.shared_store
        .insert(Arc::new(LocoJobDispatcher) as Arc<dyn EmbeddingSyncDispatcher>);
    ctx.shared_store
        .insert(Arc::new(LocoJobDispatcher) as Arc<dyn ReindexDispatcher>);
}

pub(crate) async fn replace_provider(ctx: &mut AppContext) -> Result<()> {
    if ctx.config.workers.mode == loco_rs::config::WorkerMode::BackgroundQueue
        && let Some(queue) = crate::workers::queue::timezone_queue(ctx).await?
    {
        if let Some(old) = ctx.queue_provider.as_ref() {
            old.shutdown()?;
        }
        ctx.queue_provider = Some(Arc::new(queue));
    }
    Ok(())
}

struct Relay(tokio::task::JoinHandle<()>);

pub(crate) fn start_relay(ctx: &AppContext) {
    if ctx.shared_store.get_ref::<Relay>().is_some() {
        return;
    }
    let ctx_for_task = ctx.clone();
    let task = tokio::spawn(async move {
        loop {
            crate::workers::dispatch::recover_pending(&ctx_for_task).await;
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    });
    ctx.shared_store.insert(Relay(task));
}

pub(crate) async fn shutdown(ctx: &AppContext) {
    if let Some(Relay(task)) = ctx.shared_store.remove::<Relay>() {
        task.abort();
        let _ = task.await;
    }
}
