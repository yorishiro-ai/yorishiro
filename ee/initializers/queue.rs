//! Worker policies and dispatch extensions installed over the base defaults.

use std::sync::Arc;

use loco_rs::{Result, app::AppContext};

use crate::edition::ee::{data, models, workers};

pub(crate) fn install(ctx: &AppContext) -> Result<()> {
    ctx.shared_store
        .insert(Arc::new(data::plan::plan_policy_enabled()?));
    ctx.shared_store
        .insert(Arc::new(data::worker_policy::WorkerClassLimits::from_env()?));
    ctx.shared_store.insert(Arc::new(
        models::workspace_worker_classes::WorkerClassAssignmentResolver,
    )
        as Arc<dyn crate::workers::embedding_sync::WorkerClassResolver>);
    ctx.shared_store
        .insert(Arc::new(workers::queue::PlanQueuePolicy)
            as Arc<dyn crate::workers::queue::QueuePolicy>);
    ctx.shared_store
        .insert(Arc::new(crate::workers::dispatch::LocoJobDispatcher)
            as Arc<dyn workers::infer_fill::InferFillDispatcher>);
    Ok(())
}
