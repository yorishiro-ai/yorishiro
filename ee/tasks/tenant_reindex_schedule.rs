//! Operator management of a tenant's periodic reindex schedule.

use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::edition::ee::models::tenant_reindex_schedules;

pub(crate) struct TenantReindexSchedule;

#[async_trait]
impl Task for TenantReindexSchedule {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "tenant_reindex_schedule".into(),
            detail: "Inspect, set or disable scheduling: tenant_id:<uuid> action:get|set|disable [interval:PT6H] [timezone:UTC]".into(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let tenant_id = vars
            .cli_arg("tenant_id")?
            .parse::<Uuid>()
            .map_err(|_| Error::Message("tenant_id is not a valid UUID".into()))?;
        crate::models::tenant_tenants::Model::find_by_id(&ctx.db, tenant_id)
            .await
            .map_err(|error| Error::Message(error.to_string()))?;
        match vars.cli_arg("action")? {
            "get" => {}
            "set" => tenant_reindex_schedules::set(
                &ctx.db,
                tenant_id,
                vars.cli_arg("interval")?,
                vars.cli_arg("timezone").ok(),
            )
            .await
            .map_err(|error| Error::Message(error.to_string()))?,
            "disable" => tenant_reindex_schedules::clear(&ctx.db, tenant_id)
                .await
                .map_err(|error| Error::Message(error.to_string()))?,
            _ => return Err(Error::Message("action must be get, set or disable".into())),
        }
        let schedule = tenant_reindex_schedules::get(&ctx.db, tenant_id)
            .await
            .map_err(|error| Error::Message(error.to_string()))?;
        println!(
            "{}",
            serde_json::json!({
                "tenant_id": tenant_id,
                "schedule": schedule.map(|row| serde_json::json!({
                    "interval": row.interval, "timezone": row.timezone, "scheduled_for": row.scheduled_for,
                })),
            })
        );
        Ok(())
    }
}
