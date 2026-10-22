//! Worker policies and dispatch extensions installed over the base defaults.

use std::sync::Arc;

use loco_rs::{Result, app::AppContext};

use crate::edition::ee::{data, workers};

pub(crate) fn install(ctx: &AppContext) -> Result<()> {
    ctx.shared_store
        .insert(Arc::new(data::plan::plan_policy_enabled(&ctx.config)?));
    ctx.shared_store.insert(Arc::new(
        data::worker_policy::WorkerClassLimits::from_config(&ctx.config)?,
    ));
    ctx.shared_store
        .insert(Arc::new(workers::queue::PlanQueuePolicy)
            as Arc<dyn crate::workers::queue::QueuePolicy>);
    Ok(())
}
