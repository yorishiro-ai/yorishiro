use loco_rs::prelude::*;
use loco_rs::task::Vars;

pub(crate) struct RecoverWorkerDispatches;

#[async_trait]
impl Task for RecoverWorkerDispatches {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "recover_worker_dispatches".to_string(),
            detail: "Recovers queued worker dispatches".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, _vars: &Vars) -> Result<()> {
        crate::workers::dispatch::recover_pending(ctx).await;
        Ok(())
    }
}
