//! Shared task supervision for Loco queue providers, not a second job execution system.

use std::{collections::HashMap, sync::Arc, time::Duration};

use loco_rs::{
    Result,
    bgworker::{Job, JobHandler, QueueProvider},
    config::ReaperConfig,
    prelude::async_trait,
};
use tokio::{
    sync::{Mutex, watch},
    task::JoinSet,
};

pub(super) type Handlers = Arc<Mutex<HashMap<String, Arc<JobHandler>>>>;

pub(super) struct Delivery {
    pub(super) job: Job,
    pub(super) queue: Option<String>,
}

#[async_trait]
pub(super) trait Backend: QueueProvider + Clone + 'static {
    async fn claim(
        &self,
        tags: &[String],
        stop: &mut watch::Receiver<bool>,
    ) -> Result<Option<Delivery>>;
    async fn complete(&self, delivery: &Delivery) -> Result<()>;
    async fn fail(&self, delivery: &Delivery, error: &loco_rs::Error) -> Result<()>;
}

pub(super) async fn run<B: Backend>(
    backend: B,
    handlers: Handlers,
    workers: u32,
    poll: Duration,
    reaper: Option<ReaperConfig>,
    stop: watch::Sender<bool>,
    tags: Vec<String>,
) -> Result<()> {
    let mut tasks = JoinSet::new();
    if let Some(reaper) = reaper {
        let backend = backend.clone();
        let mut stop = stop.subscribe();
        tasks.spawn(async move {
            while pause(&mut stop, Duration::from_secs(reaper.interval_seconds)).await {
                if let Err(error) = backend.requeue(&reaper.age_minutes).await {
                    tracing::error!(error = %error, "queue reaper failed");
                }
            }
        });
    }
    for worker_index in 0..workers {
        let backend = backend.clone();
        let handlers = handlers.clone();
        let tags = tags.clone();
        let mut stop = stop.subscribe();
        tasks.spawn(async move {
            while !*stop.borrow() {
                match backend.claim(&tags, &mut stop).await {
                    Ok(Some(delivery)) => {
                        // Release the registry lock before awaiting a handler, or workers serialize.
                        let handler = handlers.lock().await.get(&delivery.job.name).cloned();
                        if let Some(handler) = handler {
                            let result = handler(delivery.job.id.clone(), delivery.job.data.clone()).await;
                            let update = match result {
                                Ok(()) => backend.complete(&delivery).await,
                                Err(error) => backend.fail(&delivery, &error).await,
                            };
                            if let Err(error) = update {
                                tracing::error!(error = %error, job_id = %delivery.job.id, "queue outcome update failed");
                            }
                        } else {
                            tracing::error!(worker_index, job_name = %delivery.job.name, "no handler registered for job");
                            if !pause(&mut stop, poll).await { break; }
                        }
                    }
                    Ok(None) => if !pause(&mut stop, poll).await { break; },
                    Err(_) if *stop.borrow() => break,
                    Err(error) => {
                        tracing::error!(error = %error, "queue dequeue failed");
                        if !pause(&mut stop, Duration::from_secs(1)).await { break; }
                    }
                }
            }
        });
    }
    // Completion order matters: a later worker panic must not hide behind an earlier busy worker.
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(()) => {}
            Err(error) if error.is_cancelled() => tracing::debug!("queue task cancelled"),
            Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
            Err(error) => return Err(loco_rs::Error::Message(error.to_string())),
        }
    }
    Ok(())
}

async fn pause(stop: &mut watch::Receiver<bool>, duration: Duration) -> bool {
    if *stop.borrow() {
        return false;
    }
    tokio::select! {
        biased;
        changed = stop.changed() => changed.is_ok() && !*stop.borrow(),
        () = tokio::time::sleep(duration) => true,
    }
}
