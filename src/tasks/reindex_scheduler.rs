//! Schedules automatic reindexing across the deployment.

use loco_rs::prelude::*;
use loco_rs::{config::Config, task::Vars};

pub struct ReindexScheduler;

const NAME: &str = "reindex_scheduler";
const DEFAULT_CRON: &str = "0 * * * * *";

#[async_trait]
impl Task for ReindexScheduler {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: NAME.into(),
            detail: "Enqueue needed embedding reindexes across the deployment".into(),
        }
    }

    async fn run(&self, ctx: &AppContext, _vars: &Vars) -> Result<()> {
        crate::workers::startup_reindex::run_metadata_scan(ctx).await
    }
}

pub(crate) fn configure(config: &mut Config, schedule: Option<&str>) {
    let scheduler = config
        .scheduler
        .get_or_insert_with(|| loco_rs::scheduler::Config {
            jobs: std::collections::HashMap::new(),
            output: loco_rs::scheduler::Output::default(),
        });
    scheduler.jobs.insert(
        NAME.into(),
        loco_rs::scheduler::Job {
            run: NAME.into(),
            shell: false,
            run_on_start: schedule.is_none(),
            cron: schedule.unwrap_or(DEFAULT_CRON).into(),
            tags: None,
            output: None,
        },
    );
}

pub(crate) fn runs_immediately(config: &Config) -> bool {
    config
        .scheduler
        .as_ref()
        .and_then(|scheduler| scheduler.jobs.get(NAME))
        .is_none_or(|job| job.run_on_start)
}
