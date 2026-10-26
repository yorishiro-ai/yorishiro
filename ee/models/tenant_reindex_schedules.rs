//! Per-tenant reindex scheduling configuration.
//!
//! Reads and writes go through `ctx.db` (the migration-role connection),
//! not the RLS-scoped tenant pool: `yorishiro_app` has no GRANT on this table,
//! matching `workspace_embedding_keys` and `workspace_llm_keys`.
//!
//! A scheduler row tells the deployment-wide Loco scheduled task
//! to enqueue a reindex for every workspace under this tenant on each tick.
//! A tenant with no row is not scheduled.
//!
//! # Task
//!
//! `TenantReindexScheduler` is a loco task that runs on a cron schedule (configured via
//! `config/*.yaml` scheduler section). On each invocation it scans all scheduled tenants,
//! enqueues a reindex for every workspace under each tenant that needs one.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::tenant_reindex_schedules::{ActiveModel, Column, Entity};
use loco_rs::prelude::*;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

/// The shortest interval a schedule may name, so a typo cannot make every workspace of a tenant reindex every few seconds.
const MIN_INTERVAL_SECONDS: i64 = 5 * 60;

/// Reads an ISO 8601 duration made of weeks, days, hours, minutes and seconds, such as `P1D`, `P1W` or `PT6H`.
///
/// Years and months are refused: they have no fixed length, and a schedule that drifts with the calendar is not what an interval promises.
fn parse_interval(value: &str) -> Option<chrono::Duration> {
    const DATE_UNITS: [(char, i64); 2] = [('W', 604_800), ('D', 86_400)];
    const TIME_UNITS: [(char, i64); 3] = [('H', 3_600), ('M', 60), ('S', 1)];

    fn seconds_in(part: &str, units: &[(char, i64)]) -> Option<(i64, usize)> {
        let (mut total, mut components) = (0_i64, 0);
        let mut number = String::new();
        let mut seen = Vec::new();
        for ch in part.chars() {
            if ch.is_ascii_digit() {
                number.push(ch);
                continue;
            }
            let factor = units.iter().find(|(unit, _)| *unit == ch)?.1;
            if seen.contains(&ch) {
                return None;
            }
            seen.push(ch);
            total = total.checked_add(number.parse::<i64>().ok()?.checked_mul(factor)?)?;
            number.clear();
            components += 1;
        }
        number.is_empty().then_some((total, components))
    }

    let rest = value.trim().strip_prefix('P')?;
    let (date, time) = rest.split_once('T').unwrap_or((rest, ""));
    let (date_seconds, date_components) = seconds_in(date, &DATE_UNITS)?;
    let (time_seconds, time_components) = seconds_in(time, &TIME_UNITS)?;
    let seconds = date_seconds.checked_add(time_seconds)?;
    (date_components + time_components > 0 && seconds > 0)
        .then(|| chrono::Duration::try_seconds(seconds))?
}

/// Stores or replaces a tenant's reindex schedule.
#[allow(clippy::too_many_arguments)]
///
/// # Errors
/// Returns an error if the operation cannot be completed.
///
/// # Panics
/// Panics if an internal invariant required by this operation is violated.
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

    let Some(every) = parse_interval(interval) else {
        return Err(YorishiroError::ValidationFailed {
            message: format!("interval {interval:?} is not a supported ISO 8601 duration"),
            details: vec![],
            hint: "Use weeks, days, hours, minutes or seconds, e.g. P1D for daily, P1W for weekly or PT6H. Years and months are not supported.".into(),
        });
    };
    if every < chrono::Duration::seconds(MIN_INTERVAL_SECONDS) {
        return Err(YorishiroError::ValidationFailed {
            message: format!("interval {interval:?} is shorter than 5 minutes"),
            details: vec![],
            hint: "The shortest supported interval is PT5M.".into(),
        });
    }

    // The first run is one interval from now, and each tick schedules the next one interval after itself.
    let expected: chrono::DateTime<chrono::FixedOffset> = crate::time::now()
        .checked_add_signed(every)
        .ok_or_else(|| YorishiroError::ValidationFailed {
            message: format!("interval {interval:?} is too long"),
            details: vec![],
            hint: "Use an interval of at most a few years.".into(),
        })?
        .into();

    let active = ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        interval: ActiveValue::Set(interval.to_string()),
        timezone: ActiveValue::Set(timezone.map(|s| s.trim().to_string())),
        scheduled_for: ActiveValue::Set(Some(expected)),
        updated_at: ActiveValue::Set(crate::time::now().into()),
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

/// The schedule row itself, as the ticker reads it.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn get(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<Option<crate::models::_entities::tenant_reindex_schedules::Model>, YorishiroError> {
    let row = Entity::find()
        .filter(Column::TenantId.eq(tenant_id))
        .one(conn)
        .await
        .internal()?;

    Ok(row)
}

/// Advances due schedules and dispatches reindex jobs while holding scheduler ownership.
///
/// # Errors
/// Returns database, ownership or dispatch failures.
pub(crate) async fn tick(
    app_context: &AppContext,
    tick: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    use crate::db::AppContextBackend;
    use crate::models::_entities::tenant_reindex_schedules as ScheduleEntity;
    use crate::models::_entities::workspace_workspaces as WorkspaceEntity;
    use crate::workers::reindex::ReindexArgs;

    // Ownership serializes ticks; the transaction records schedules and outbox rows together.
    let ownership = match crate::edition::ee::db::acquire_scheduler_ownership(
        app_context,
        "yorishiro:tenant-reindex-scheduler",
    )
    .await
    {
        Ok(Some(ownership)) => ownership,
        Ok(None) => {
            tracing::info!(
                ownership_key = "yorishiro:tenant-reindex-scheduler",
                "reindex scheduler tick skipped because another owner holds scheduler ownership"
            );
            return Ok(());
        }
        Err(err) => {
            return Err(Error::Message(format!(
                "scheduler ownership acquisition: {err}"
            )));
        }
    };
    tracing::info!(
        ownership_key = "yorishiro:tenant-reindex-scheduler",
        ownership_backend = if app_context.is_postgres() {
            "postgres"
        } else {
            "sqlite"
        },
        ownership_lock_path = ownership
            .sqlite_path()
            .map(|path| path.display().to_string()),
        "reindex scheduler tick ownership acquired"
    );

    let txn = match crate::db::begin_write(&app_context.db).await {
        Ok(txn) => txn,
        Err(err) => {
            let _ = release_scheduler_ownership(ownership).await;
            return Err(Error::Message(format!(
                "scheduler ownership transaction: {err}"
            )));
        }
    };
    let schedules = match ScheduleEntity::Entity::find().all(&txn).await {
        Ok(schedules) => schedules,
        Err(err) => {
            let _ = txn.rollback().await;
            let _ = release_scheduler_ownership(ownership).await;
            return Err(Error::Message(err.to_string()));
        }
    };

    let mut dispatches = Vec::new();
    for schedule in &schedules {
        // Only enqueue once the scheduled time has arrived.
        let sched_utc = match schedule.scheduled_for {
            Some(v) => chrono::DateTime::<chrono::Utc>::from(v),
            None => continue,
        };
        if !is_due(sched_utc, tick) {
            continue;
        }
        let next = parse_interval(&schedule.interval)
            .filter(|every| every.num_seconds() >= MIN_INTERVAL_SECONDS)
            .and_then(|every| tick.checked_add_signed(every));
        let Some(next) = next else {
            let _ = txn.rollback().await;
            let release = release_scheduler_ownership(ownership).await;
            return Err(Error::Message(format!(
                "invalid stored reindex interval for tenant {}: {:?}; ownership release: {release:?}",
                schedule.tenant_id, schedule.interval
            )));
        };

        // Fetch all workspaces under this tenant.
        let workspaces: Vec<_> = match WorkspaceEntity::Entity::find()
            .filter(WorkspaceEntity::Column::TenantId.eq(schedule.tenant_id))
            .all(&txn)
            .await
        {
            Ok(workspaces) => workspaces,
            Err(err) => {
                let _ = txn.rollback().await;
                let _ = release_scheduler_ownership(ownership).await;
                return Err(Error::Message(err.to_string()));
            }
        };

        for ws in workspaces {
            let worker_class = match crate::controllers::extractors::resolve_worker_class(
                app_context,
                ws.id,
            )
            .await
            {
                Ok(worker_class) => worker_class,
                Err(err) => {
                    let _ = txn.rollback().await;
                    let release = release_scheduler_ownership(ownership).await;
                    return Err(Error::Message(format!(
                        "reindex scheduler: workspace {} target resolution failed: {}; ownership release: {release:?}",
                        ws.id, err.0
                    )));
                }
            };
            let args = ReindexArgs {
                lifecycle_id: None,
                workspace_id: ws.id,
                worker_class,
                startup: false,
            };
            dispatches.push((schedule.tenant_id, args));
        }

        // This update commits only if every durable dispatch is recorded successfully.
        let mut active = schedule.clone().into_active_model();
        active.scheduled_for = ActiveValue::Set(Some(next.into()));
        active.updated_at = ActiveValue::Set(tick.into());
        if let Err(err) = active.update(&txn).await {
            let _ = txn.rollback().await;
            let _ = release_scheduler_ownership(ownership).await;
            return Err(Error::Message(err.to_string()));
        }
    }

    let dispatch_errors = dispatch_reindex_batch(app_context, dispatches, Some(&txn)).await;
    if !dispatch_errors.is_empty() {
        let _ = txn.rollback().await;
        let release_error = release_scheduler_ownership(ownership).await;
        return Err(Error::Message(
            scheduler_failure_message(
                &dispatch_errors,
                release_error.as_ref().err().map(String::as_str),
            )
            .unwrap_or_else(|| "reindex scheduler durable dispatch failed".into()),
        ));
    }
    if let Err(err) = txn.commit().await {
        let _ = release_scheduler_ownership(ownership).await;
        return Err(Error::Message(format!("scheduler ownership commit: {err}")));
    }

    let release_error = release_scheduler_ownership(ownership).await;
    if let Some(message) = scheduler_failure_message(
        &dispatch_errors,
        release_error.as_ref().err().map(String::as_str),
    ) {
        tracing::error!(error = %message, "reindex scheduler tick failed after dispatch attempts");
        return Err(Error::Message(message));
    }
    Ok(())
}

pub fn scheduler_failure_message(
    dispatch_errors: &[String],
    release_error: Option<&str>,
) -> Option<String> {
    if dispatch_errors.is_empty() {
        return release_error
            .map(|err| format!("reindex scheduler ownership release failed: {err}"));
    }

    let mut message = format!(
        "reindex scheduler dispatch failed for {} workspace(s); schedule advancement rolled back: {}",
        dispatch_errors.len(),
        dispatch_errors.join("; ")
    );
    if let Some(err) = release_error {
        message.push_str(&format!("; scheduler ownership release also failed: {err}"));
    }
    Some(message)
}

pub async fn dispatch_reindex_batch(
    app_context: &AppContext,
    dispatches: Vec<(Uuid, crate::workers::reindex::ReindexArgs)>,
    txn: Option<&sea_orm::DatabaseTransaction>,
) -> Vec<String> {
    let mut dispatch_errors = Vec::new();
    for (tenant_id, args) in dispatches {
        let workspace_id = args.workspace_id;
        if let Err(err) = crate::workers::dispatch::dispatch_reindex(app_context, args, txn).await {
            tracing::warn!(
                tenant_id = %tenant_id,
                workspace_id = %workspace_id,
                error = %err,
                "reindex scheduler: durable dispatch failed"
            );
            dispatch_errors.push(format!("workspace {workspace_id}: {err}"));
        } else {
            tracing::info!(
                tenant_id = %tenant_id,
                workspace_id = %workspace_id,
                "reindex scheduler: dispatched"
            );
        }
    }
    dispatch_errors
}

async fn release_scheduler_ownership(
    ownership: crate::edition::ee::db::SchedulerOwnership,
) -> Result<(), String> {
    match ownership.release().await {
        Ok(()) => {
            tracing::info!(
                ownership_key = "yorishiro:tenant-reindex-scheduler",
                "reindex scheduler tick ownership released"
            );
            Ok(())
        }
        Err(err) => {
            tracing::error!(
                ownership_key = "yorishiro:tenant-reindex-scheduler",
                error = %err,
                "reindex scheduler tick ownership release failed"
            );
            Err(err)
        }
    }
}

pub fn is_due(
    scheduled_for: chrono::DateTime<chrono::Utc>,
    tick: chrono::DateTime<chrono::Utc>,
) -> bool {
    scheduled_for <= tick
}
