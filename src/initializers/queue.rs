//! Queue provider, dispatcher and relay lifecycle wiring.

use loco_rs::app::AppContext;

pub(crate) fn install(ctx: &AppContext) {
    ctx.shared_store
        .insert(crate::workers::queue::default_queue_policy());
}

struct Relay {
    stop: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

pub(crate) fn start_relay(ctx: &AppContext) -> loco_rs::Result<()> {
    if ctx.shared_store.get_ref::<Relay>().is_some() {
        return Ok(());
    }
    let settings = ctx.config.settings::<crate::data::settings::Settings>()?;
    let poll_interval = std::time::Duration::from_millis(settings.dispatch.poll_interval_ms);
    let ctx_for_task = ctx.clone();
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        loop {
            if *stopped.borrow() {
                break;
            }
            crate::workers::dispatch::recover_pending_while(&ctx_for_task, || !*stopped.borrow())
                .await;
            tokio::select! {
                _ = stopped.changed() => break,
                _ = tokio::time::sleep(poll_interval) => {}
            }
        }
    });
    ctx.shared_store.insert(Relay { stop, task });
    Ok(())
}

pub(crate) async fn shutdown(ctx: &AppContext) {
    if let Some(Relay { stop, task }) = ctx.shared_store.remove::<Relay>() {
        let _ = stop.send(true);
        if let Err(error) = task.await {
            tracing::error!(%error, "dispatch relay failed during shutdown");
        }
    }
}
