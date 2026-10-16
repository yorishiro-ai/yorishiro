use loco_rs::prelude::*;
use loco_rs::task::Vars;

pub(crate) struct ReplayQueueDispatchSnapshot;

#[async_trait]
impl Task for ReplayQueueDispatchSnapshot {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "replay_queue_dispatch_snapshot".into(),
            detail:
                "Replay durable dispatches after old-worker exit and provider snapshot attestation"
                    .into(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let expected_epoch: i64 = vars
            .cli_arg("expected_epoch")?
            .parse()
            .map_err(|e| loco_rs::Error::Message(format!("invalid expected_epoch: {e}")))?;
        let snapshot_id: &str = vars.cli_arg("snapshot_id")?;
        let old_workers_exited: bool = vars
            .cli_arg("old_workers_exited")?
            .parse()
            .map_err(|e| loco_rs::Error::Message(format!("invalid old_workers_exited: {e}")))?;
        let phase: &str = vars.cli_arg("phase")?;
        let cutoff_at: Option<chrono::DateTime<chrono::FixedOffset>> = vars
            .cli_arg("cutoff_at")
            .ok()
            .map(|value: &str| value.parse())
            .transpose()
            .map_err(|e| loco_rs::Error::Message(format!("invalid cutoff_at: {e}")))?;
        if !old_workers_exited
            || snapshot_id.trim().is_empty()
            || !matches!(phase, "prepare" | "activate")
        {
            return Err(loco_rs::Error::Message(
                "phase must be prepare or activate, with old-worker-exit attestation and provider snapshot id".into(),
            ));
        }
        let handle = ctx.shared_store.get::<crate::db::DbHandle>();
        let db = handle
            .as_ref()
            .map(crate::db::DbHandle::identity_connection)
            .unwrap_or_else(|| ctx.db.clone());
        let txn = db
            .begin()
            .await
            .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
        if phase == "activate" {
            let saved_state = crate::models::queue_job_dispatch_outbox::publishing_state(&txn)
                .await
                .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
            if saved_state.active
                || saved_state.epoch != expected_epoch + 1
                || saved_state.cutoff_at.is_none()
            {
                return Err(loco_rs::Error::Message(
                    "activation requires the expected paused epoch with a prepared cutoff".into(),
                ));
            }
            // Validate the caller's evidence token against the token recorded at prepare.
            if saved_state.snapshot_id.as_deref() != Some(snapshot_id) {
                return Err(loco_rs::Error::Message(
                    "snapshot_id does not match prepared evidence".into(),
                ));
            }
        }
        let count = if phase == "prepare" {
            let cutoff_at = cutoff_at
                .ok_or_else(|| loco_rs::Error::Message("prepare requires cutoff_at".into()))?;
            crate::models::queue_job_dispatch_outbox::prepare_replay_snapshot(
                &txn,
                expected_epoch,
                cutoff_at,
                old_workers_exited,
                snapshot_id,
            )
            .await
            .map_err(|e| loco_rs::Error::Message(e.to_string()))?
        } else {
            if cutoff_at.is_some() {
                return Err(loco_rs::Error::Message(
                    "activate does not accept cutoff_at".into(),
                ));
            }
            crate::models::queue_job_dispatch_outbox::activate_prepared_publishing_epoch(
                &txn,
                expected_epoch,
                snapshot_id,
            )
            .await
            .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
            0
        };
        txn.commit()
            .await
            .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
        println!(
            "phase={phase}; retagged {count} durable dispatch rows for provider snapshot {snapshot_id}"
        );
        Ok(())
    }
}
