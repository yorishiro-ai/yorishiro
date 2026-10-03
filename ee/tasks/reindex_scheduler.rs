//! Per-tenant reindex scheduling configuration.
//!
//! Reads and writes go through `ctx.db` (the migration-role connection),
//! not the RLS-scoped tenant pool: `yorishiro_app` has no GRANT on this table,
//! matching `workspace_embedding_keys` and `workspace_llm_keys`.
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
use crate::models::_entities::tenant_reindex_schedules::{ActiveModel, Column, Entity};
use loco_rs::prelude::*;
use loco_rs::task::Vars;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, TransactionTrait,
};
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

    // Compute the next scheduled time: now + 5 minutes.
    // This is the interval until the next run, not a grace period for a missed run.
    //
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
) -> Result<Option<crate::models::_entities::tenant_reindex_schedules::Model>, YorishiroError> {
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
pub struct TenantReindexScheduler;

#[async_trait]
impl Task for TenantReindexScheduler {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "tenant_reindex_scheduler".to_string(),
            detail: "Enqueue reindex jobs for scheduled tenants".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        self.run_at(app_context, vars, chrono::Utc::now()).await
    }
}

impl TenantReindexScheduler {
    async fn run_at(
        &self,
        app_context: &AppContext,
        _vars: &Vars,
        tick: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        use crate::db::AppContextBackend;
        use crate::models::_entities::tenant_reindex_schedules as ScheduleEntity;
        use crate::models::_entities::workspace_workspaces as WorkspaceEntity;
        use crate::workers::embedding_sync::WorkerClass;
        use crate::workers::reindex::ReindexArgs;

        // Ownership is held on a detached PostgreSQL session or an OS file handle through every
        // queue dispatch. The schedule transaction is separate so dispatch cannot hold a DB
        // transaction open, and the guard is released explicitly after the final dispatch.
        let mut ownership = match crate::ee::db::acquire_scheduler_ownership(
            app_context,
            "yorishiro:tenant-reindex-scheduler",
        )
        .await
        {
            Ok(Some(ownership)) => Some(ownership),
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
                .as_ref()
                .and_then(crate::ee::db::SchedulerOwnership::sqlite_path)
                .map(|path| path.display().to_string()),
            "reindex scheduler tick ownership acquired"
        );

        let txn = match app_context.db.begin().await {
            Ok(txn) => txn,
            Err(err) => {
                let _ = release_scheduler_ownership(ownership.take().unwrap()).await;
                return Err(Error::Message(format!(
                    "scheduler ownership transaction: {err}"
                )));
            }
        };
        let schedules = match ScheduleEntity::Entity::find().all(&txn).await {
            Ok(schedules) => schedules,
            Err(err) => {
                let _ = txn.rollback().await;
                let _ = release_scheduler_ownership(ownership.take().unwrap()).await;
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

            // Fetch all workspaces under this tenant.
            let workspaces: Vec<_> = match WorkspaceEntity::Entity::find()
                .filter(WorkspaceEntity::Column::TenantId.eq(schedule.tenant_id))
                .all(&txn)
                .await
            {
                Ok(workspaces) => workspaces,
                Err(err) => {
                    let _ = txn.rollback().await;
                    let _ = release_scheduler_ownership(ownership.take().unwrap()).await;
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
                        tracing::warn!(
                            workspace_id = %ws.id,
                            error = %err.0,
                            "reindex scheduler: failed to resolve worker class, defaulting to shared"
                        );
                        WorkerClass::Shared
                    }
                };
                let args = ReindexArgs {
                    lifecycle_id: None,
                    workspace_id: ws.id,
                    worker_class,
                };
                dispatches.push((schedule.tenant_id, args));
            }

            // Advance before dispatch for the deliberate at-most-once policy. A crash or queue
            // failure after this commit loses the current interval until the next one.
            let mut active = schedule.clone().into_active_model();
            let new_sched: chrono::DateTime<chrono::FixedOffset> = tick
                .checked_add_signed(chrono::Duration::minutes(5))
                .unwrap()
                .into();
            active.scheduled_for = ActiveValue::Set(Some(new_sched));
            active.updated_at = ActiveValue::Set(tick.into());
            if let Err(err) = active.update(&txn).await {
                let _ = txn.rollback().await;
                let _ = release_scheduler_ownership(ownership.take().unwrap()).await;
                return Err(Error::Message(err.to_string()));
            }
        }

        if let Err(err) = txn.commit().await {
            let _ = release_scheduler_ownership(ownership.take().unwrap()).await;
            return Err(Error::Message(format!("scheduler ownership commit: {err}")));
        }

        let dispatch_errors = dispatch_reindex_batch(app_context, dispatches).await;
        let release_error = release_scheduler_ownership(ownership.take().unwrap()).await;
        if let Some(message) = scheduler_failure_message(
            &dispatch_errors,
            release_error.as_ref().err().map(String::as_str),
        ) {
            tracing::error!(error = %message, "reindex scheduler tick failed after dispatch attempts");
            return Err(Error::Message(message));
        }
        Ok(())
    }
}

fn scheduler_failure_message(
    dispatch_errors: &[String],
    release_error: Option<&str>,
) -> Option<String> {
    if dispatch_errors.is_empty() {
        return release_error
            .map(|err| format!("reindex scheduler ownership release failed: {err}"));
    }

    let mut message = format!(
        "reindex scheduler dispatch failed for {} workspace(s) after schedule advancement; interval is not retried: {}",
        dispatch_errors.len(),
        dispatch_errors.join("; ")
    );
    if let Some(err) = release_error {
        message.push_str(&format!("; scheduler ownership release also failed: {err}"));
    }
    Some(message)
}

async fn dispatch_reindex_batch(
    app_context: &AppContext,
    dispatches: Vec<(Uuid, crate::workers::reindex::ReindexArgs)>,
) -> Vec<String> {
    let mut dispatch_errors = Vec::new();
    for (tenant_id, args) in dispatches {
        let workspace_id = args.workspace_id;
        if let Err(err) = crate::workers::reindex::enqueue_for_class(app_context, args).await {
            tracing::warn!(
                tenant_id = %tenant_id,
                workspace_id = %workspace_id,
                error = %err,
                "reindex scheduler: dispatch failed after schedule advancement"
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
    ownership: crate::ee::db::SchedulerOwnership,
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

fn is_due(
    scheduled_for: chrono::DateTime<chrono::Utc>,
    tick: chrono::DateTime<chrono::Utc>,
) -> bool {
    scheduled_for <= tick
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    use super::{dispatch_reindex_batch, is_due, scheduler_failure_message};

    const TICK: i64 = 1_700_000_000;

    fn at(offset: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(TICK + offset, 0).single().unwrap()
    }

    #[test]
    fn due_decision_keeps_the_exact_tick_boundary_matrix() {
        for offset in [-301, 0, 1, 300, 301] {
            assert_eq!(
                is_due(at(offset), at(0)),
                offset <= 0,
                "scheduled offset {offset}"
            );
        }
    }

    struct FailingDispatcher {
        attempts: AtomicUsize,
    }

    #[async_trait]
    impl crate::workers::dispatch::ReindexDispatcher for FailingDispatcher {
        async fn dispatch(
            &self,
            _ctx: &loco_rs::app::AppContext,
            _args: crate::workers::reindex::ReindexArgs,
        ) -> loco_rs::Result<String> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(loco_rs::Error::Message("dispatch failed".into()))
        }
    }

    #[tokio::test]
    async fn dispatch_batch_attempts_every_workspace_and_aggregates_failures() {
        let ctx = crate::workers::dispatch::test_context().await;
        let dispatcher = Arc::new(FailingDispatcher {
            attempts: AtomicUsize::new(0),
        });
        ctx.shared_store
            .insert(dispatcher.clone() as Arc<dyn crate::workers::dispatch::ReindexDispatcher>);
        let dispatches = (0..3)
            .map(|_| {
                (
                    Uuid::now_v7(),
                    crate::workers::reindex::ReindexArgs {
                        lifecycle_id: None,
                        workspace_id: Uuid::now_v7(),
                        worker_class: crate::workers::embedding_sync::WorkerClass::Shared,
                    },
                )
            })
            .collect();

        let errors = dispatch_reindex_batch(&ctx, dispatches).await;
        assert_eq!(dispatcher.attempts.load(Ordering::SeqCst), 3);
        assert_eq!(errors.len(), 3);
    }

    #[test]
    fn dispatch_failure_remains_primary_when_release_also_fails() {
        let message = scheduler_failure_message(
            &[
                "workspace one: enqueue failed".into(),
                "workspace two: enqueue failed".into(),
            ],
            Some("unlock failed"),
        )
        .unwrap();

        assert!(message.starts_with("reindex scheduler dispatch failed for 2 workspace(s)"));
        assert!(message.contains("interval is not retried"));
        assert!(message.contains("workspace one: enqueue failed"));
        assert!(message.contains("scheduler ownership release also failed: unlock failed"));
    }

    #[tokio::test]
    async fn sqlite_task_advances_schedule_before_all_failed_dispatches() {
        use loco_rs::app::AppContext;
        use loco_rs::app::Hooks;
        use migration::{Migrator, MigratorTrait};
        use sea_orm::{ActiveModelTrait, ActiveValue, Database, EntityTrait};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let path = dir.path().join("scheduler.sqlite");
        let uri = format!("sqlite://{}?mode=rwc", path.display());
        crate::db::register_sqlite_extensions();
        let db = Database::connect(&uri).await.unwrap();
        Migrator::up(&db, None).await.unwrap();
        let mut config = crate::app::App::load_config(&loco_rs::environment::Environment::Any(
            "test_sqlite".into(),
        ))
        .await
        .unwrap();
        config.database.uri = uri;
        let ctx = AppContext::builder(loco_rs::environment::Environment::Test, db, config).build();

        let tenant = crate::models::_entities::tenant_tenants::ActiveModel {
            name: ActiveValue::Set("scheduler-test".into()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        for name in ["one", "two"] {
            crate::models::_entities::workspace_workspaces::ActiveModel {
                tenant_id: ActiveValue::Set(tenant.id),
                name: ActiveValue::Set(name.into()),
                status: ActiveValue::Set(
                    crate::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE.into(),
                ),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .unwrap();
        }
        let initial = at(-1);
        crate::models::_entities::tenant_reindex_schedules::ActiveModel {
            tenant_id: ActiveValue::Set(tenant.id),
            interval: ActiveValue::Set("P1D".into()),
            scheduled_for: ActiveValue::Set(Some(initial.into())),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();

        let dispatcher = Arc::new(FailingDispatcher {
            attempts: AtomicUsize::new(0),
        });
        ctx.shared_store
            .insert(dispatcher.clone() as Arc<dyn crate::workers::dispatch::ReindexDispatcher>);
        let task = super::TenantReindexScheduler;
        let first = task
            .run_at(&ctx, &loco_rs::task::Vars::default(), at(0))
            .await;
        assert!(first.is_err());
        assert_eq!(dispatcher.attempts.load(Ordering::SeqCst), 2);

        let schedule =
            crate::models::_entities::tenant_reindex_schedules::Entity::find_by_id(tenant.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
        let expected: chrono::DateTime<chrono::FixedOffset> = at(300).into();
        assert_eq!(schedule.scheduled_for.unwrap(), expected);

        let second = task
            .run_at(&ctx, &loco_rs::task::Vars::default(), at(0))
            .await;
        assert!(second.is_ok());
        assert_eq!(dispatcher.attempts.load(Ordering::SeqCst), 2);
        let ownership =
            crate::ee::db::acquire_scheduler_ownership(&ctx, "yorishiro:tenant-reindex-scheduler")
                .await
                .unwrap()
                .unwrap();
        ownership.release().await.unwrap();
    }
}
