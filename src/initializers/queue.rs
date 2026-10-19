//! Queue provider, dispatcher and relay lifecycle wiring.

use std::sync::Arc;

use loco_rs::app::AppContext;

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

struct Relay(tokio::task::JoinHandle<()>);

pub(crate) fn start_relay(ctx: &AppContext) -> loco_rs::Result<()> {
    if ctx.shared_store.get_ref::<Relay>().is_some() {
        return Ok(());
    }
    let settings = ctx.config.settings::<crate::data::settings::Settings>()?;
    let poll_interval = std::time::Duration::from_millis(settings.dispatch.poll_interval_ms);
    let ctx_for_task = ctx.clone();
    let task = tokio::spawn(async move {
        loop {
            crate::workers::dispatch::recover_pending(&ctx_for_task).await;
            tokio::time::sleep(poll_interval).await;
        }
    });
    ctx.shared_store.insert(Relay(task));
    Ok(())
}

pub(crate) async fn shutdown(ctx: &AppContext) {
    if let Some(Relay(task)) = ctx.shared_store.remove::<Relay>() {
        task.abort();
        let _ = task.await;
    }
}
