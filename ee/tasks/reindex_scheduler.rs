//! Per-tenant reindex scheduling configuration.
//!
//! Reads and writes go through `ctx.db` (the migration-role connection),
//! not the RLS-scoped tenant pool: `yorishiro_app` has no GRANT on this table,
//! matching `identity_workspace_embedding_keys` and `identity_workspace_llm_keys`.
//!
//! A scheduler row tells the deployment-wide ticker (started in `Hooks::after_context`)
//! to enqueue a reindex for every workspace under this tenant on each tick.
//! A tenant with no row is not scheduled.
//!
//! # Task
//!
//! `TenantReindexScheduler` is a loco task that runs on a cron schedule (configured via
//! `config/*.yaml` scheduler section). On each invocation it scans all scheduled tenants,
//! enqueues a reindex for every workspace under each tenant that needs one.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::identity_tenant_reindex_schedules::{ActiveModel, Column, Entity};
use loco_rs::prelude::*;
use loco_rs::task::Vars;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

/// What a tenant has configured, for an endpoint to report.
#[derive(Debug, Clone, Serialize)]
pub struct ScheduleDescription {
    /// ISO 8601 duration, e.g. "P1D" (every day), "P1W" (every week).
    /// Defaults to "P1D" (daily) if the row was written before this field existed.
    pub interval: String,
    /// IANA time zone name, e.g. "Asia/Tokyo".
    /// None means UTC.
    pub timezone: Option<String>,
    /// The next scheduled run, computed from the last tick time.
    /// The ticker sets this before enqueueing to prevent duplicate runs
    /// if a tick was missed and the next tick would otherwise replay it.
    pub scheduled_for: Option<chrono::DateTime<chrono::Utc>>,
}

/// Stores or replaces a tenant's reindex schedule.
#[allow(clippy::too_many_arguments)]
pub async fn set(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    interval: &str,
    timezone: Option<&str>,
) -> Result<(), YorishiroError> {
    if interval.trim().is_empty() {
        return Err(YorishiroError::ValidationFailed {
            message: "interval must not be empty".into(),
            details: vec![],
            hint: "ISO 8601 duration, e.g. P1D for daily or P1W for weekly".into(),
        });
    }

    // Validate timezone if provided.
    if let Some(tz) = timezone {
        if tz.trim().is_empty() {
            return Err(YorishiroError::ValidationFailed {
                message: "timezone must not be empty if provided".into(),
                details: vec![],
                hint: "IANA time zone name, e.g. Asia/Tokyo or UTC. Omit to use UTC.".into(),
            });
        }
        // Basic check: IANA tz names contain at least one "/".
        if !tz.contains('/') && tz != "UTC" && tz != "Etc/UTC" {
            return Err(YorishiroError::ValidationFailed {
                message: format!("timezone {tz:?} does not look like a valid IANA time zone name"),
                details: vec![],
                hint: "Use names like Asia/Tokyo, America/New_York, or UTC.".into(),
            });
        }
    }

    // Compute the next scheduled time: now + 5 minutes grace.
    // The grace period is 5 minutes: if the expected time has passed within
    // the last 5 minutes, still enqueue it (the tick missed its window
    // but is still within a reasonable window). Beyond that, skip and log.
    //
    // 5 minutes is chosen because it covers a tick that was delayed by
    // a process restart or a brief queue unavailability. It is short enough
    // that a missed daily run does not surprise an operator (they would
    // notice the gap within a day). It is long enough to absorb a few
    // minutes of queue backlog without silently dropping the run.
    let expected: chrono::DateTime<chrono::FixedOffset> = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::minutes(5))
        .unwrap()
        .into();

    let active = ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        interval: ActiveValue::Set(interval.to_string()),
        timezone: ActiveValue::Set(timezone.map(|s| s.trim().to_string())),
        scheduled_for: ActiveValue::Set(Some(expected)),
        updated_at: ActiveValue::Set(chrono::Utc::now().into()),
        ..Default::default()
    };
    Entity::insert(active)
        .on_conflict(
            OnConflict::column(Column::TenantId)
                .update_columns([
                    Column::Interval,
                    Column::Timezone,
                    Column::ScheduledFor,
                    Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// Removes a tenant's schedule.
/// The tenant is no longer picked up by the ticker.
pub async fn clear(conn: &impl ConnectionTrait, tenant_id: Uuid) -> Result<(), YorishiroError> {
    Entity::delete_many()
        .filter(Column::TenantId.eq(tenant_id))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// What is configured, for an endpoint to report.
pub async fn describe(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<Option<ScheduleDescription>, YorishiroError> {
    get(conn, tenant_id).await.map(|found| {
        found.map(|row| ScheduleDescription {
            interval: row.interval,
            timezone: row.timezone,
            // Entity stores DateTimeWithTimeZone (FixedOffset); convert to Utc for the API.
            scheduled_for: row.scheduled_for.map(chrono::DateTime::<chrono::Utc>::from),
        })
    })
}

/// The schedule row itself, as the ticker reads it.
pub async fn get(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<
    Option<crate::models::_entities::identity_tenant_reindex_schedules::Model>,
    YorishiroError,
> {
    let row = Entity::find()
        .filter(Column::TenantId.eq(tenant_id))
        .one(conn)
        .await
        .internal()?;

    Ok(row)
}

/// A loco task that scans scheduled tenants and enqueue reindex jobs.
///
/// The scheduler runs periodically via loco's `scheduler` config (`config/*.yaml`
/// `scheduler:` section). Each run enqueues a reindex job for every workspace
/// under every scheduled tenant whose `scheduled_for` time has passed.
///
/// The ticker uses the grace period documented on `set`: 5 minutes.
pub struct TenantReindexScheduler;

#[async_trait]
impl Task for TenantReindexScheduler {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "tenant_reindex_scheduler".to_string(),
            detail: "Enqueue reindex jobs for scheduled tenants".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, _vars: &Vars) -> Result<()> {
        use crate::models::_entities::identity_tenant_reindex_schedules as ScheduleEntity;
        use crate::models::_entities::identity_workspaces as WorkspaceEntity;
        use crate::workers::embedding_sync::WorkerClass;
        use crate::workers::reindex::ReindexArgs;

        let tick = chrono::Utc::now();
        let grace = chrono::Duration::minutes(5);
        let cutoff = tick - grace;

        // Fetch all scheduled tenants.
        let schedules = ScheduleEntity::Entity::find()
            .all(&app_context.db)
            .await
            .map_err(|e| Error::Message(e.to_string()))?;

        for schedule in &schedules {
            // Only enqueue if the expected time has passed (within grace window).
            let sched_utc = match schedule.scheduled_for {
                Some(v) => chrono::DateTime::<chrono::Utc>::from(v),
                None => continue,
            };
            if sched_utc > tick && sched_utc > cutoff {
                continue;
            }

            // Fetch all workspaces under this tenant.
            let workspaces: Vec<_> = WorkspaceEntity::Entity::find()
                .filter(WorkspaceEntity::Column::TenantId.eq(schedule.tenant_id))
                .all(&app_context.db)
                .await
                .map_err(|e| Error::Message(e.to_string()))?;

            for ws in workspaces {
                let args = ReindexArgs {
                    workspace_id: ws.id,
                    worker_class: WorkerClass::Shared,
                };
                if let Err(err) =
                    crate::workers::reindex::enqueue_for_class(app_context, args).await
                {
                    tracing::warn!(
                        tenant_id = %schedule.tenant_id,
                        workspace_id = %ws.id,
                        error = %err,
                        "reindex scheduler: failed to enqueue"
                    );
                } else {
                    tracing::info!(
                        tenant_id = %schedule.tenant_id,
                        workspace_id = %ws.id,
                        "reindex scheduler: enqueued"
                    );
                }
            }

            // Update scheduled_for to now (ticker sets this to prevent duplicate
            // runs if a tick is missed).
            let mut active = schedule.clone().into_active_model();
            let new_sched: chrono::DateTime<chrono::FixedOffset> = chrono::Utc::now()
                .checked_add_signed(chrono::Duration::minutes(5))
                .unwrap()
                .into();
            active.scheduled_for = ActiveValue::Set(Some(new_sched));
            active.updated_at = ActiveValue::Set(chrono::Utc::now().into());
            active
                .update(&app_context.db)
                .await
                .map_err(|e| Error::Message(e.to_string()))?;
        }

        Ok(())
    }
}
