use loco_rs::prelude::*;
use loco_rs::task::Vars;

pub(crate) struct SetQueuePublishingEpoch;

#[async_trait]
impl Task for SetQueuePublishingEpoch {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "set_queue_publishing_epoch".into(),
            detail: "Advance or pause the global queue publisher epoch".into(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let expected: i64 = vars
            .cli_arg("expected_epoch")?
            .parse()
            .map_err(|e| loco_rs::Error::Message(format!("invalid expected_epoch: {e}")))?;
        let active: bool = vars
            .cli_arg("active")?
            .parse()
            .map_err(|e| loco_rs::Error::Message(format!("invalid active: {e}")))?;
        let cutoff = vars
            .cli_arg("cutoff_at")
            .ok()
            .map(|s: &str| s.parse())
            .transpose()
            .map_err(|e| loco_rs::Error::Message(format!("invalid cutoff_at: {e}")))?;
        let handle = ctx.shared_store.get::<crate::db::DbHandle>();
        if let Some(handle) = handle {
            let db = handle.identity_connection();
            let txn = db
                .begin()
                .await
                .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
            let epoch = crate::models::queue_job_dispatch_outbox::change_publishing_state(
                &txn, expected, active, cutoff,
            )
            .await
            .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
            txn.commit()
                .await
                .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
            println!("publisher epoch advanced to {epoch}; active={active}");
            return Ok(());
        }
        let txn = ctx
            .db
            .begin()
            .await
            .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
        let epoch = crate::models::queue_job_dispatch_outbox::change_publishing_state(
            &txn, expected, active, cutoff,
        )
        .await
        .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
        txn.commit()
            .await
            .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
        println!("publisher epoch advanced to {epoch}; active={active}");
        Ok(())
    }
}
