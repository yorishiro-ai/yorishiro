use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use uuid::Uuid;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};

use yorishiro::edition::ee::tasks::reindex_scheduler::{
    dispatch_reindex_batch, is_due, scheduler_failure_message,
};

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
impl yorishiro::workers::dispatch::ReindexDispatcher for FailingDispatcher {
    async fn dispatch(
        &self,
        _ctx: &loco_rs::app::AppContext,
        _args: yorishiro::workers::reindex::ReindexArgs,
    ) -> loco_rs::Result<String> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err(loco_rs::Error::Message("dispatch failed".into()))
    }
}

#[tokio::test]
async fn dispatch_batch_attempts_every_workspace_and_aggregates_failures() {
    let ctx = crate::workers::test_context().await;
    let dispatcher = Arc::new(FailingDispatcher {
        attempts: AtomicUsize::new(0),
    });
    ctx.shared_store
        .insert(dispatcher.clone() as Arc<dyn yorishiro::workers::dispatch::ReindexDispatcher>);
    let dispatches = (0..3)
        .map(|_| {
            (
                Uuid::now_v7(),
                yorishiro::workers::reindex::ReindexArgs {
                    lifecycle_id: None,
                    workspace_id: Uuid::now_v7(),
                    worker_class: yorishiro::workers::embedding_sync::WorkerClass::Shared,
                    startup: false,
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
    yorishiro::db::register_sqlite_extensions();
    let db = Database::connect(&uri).await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    let mut config = yorishiro::App::load_config(&loco_rs::environment::Environment::Any(
        "test_sqlite".into(),
    ))
    .await
    .unwrap();
    config.database.uri = uri;
    let ctx = AppContext::builder(loco_rs::environment::Environment::Test, db, config).build();

    let tenant = yorishiro::models::_entities::tenant_tenants::ActiveModel {
        name: ActiveValue::Set("scheduler-test".into()),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .unwrap();
    for name in ["one", "two"] {
        yorishiro::models::_entities::workspace_workspaces::ActiveModel {
            tenant_id: ActiveValue::Set(tenant.id),
            name: ActiveValue::Set(name.into()),
            status: ActiveValue::Set(
                yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE.into(),
            ),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
    }
    let initial = at(-1);
    yorishiro::models::_entities::tenant_reindex_schedules::ActiveModel {
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
        .insert(dispatcher.clone() as Arc<dyn yorishiro::workers::dispatch::ReindexDispatcher>);
    let task = yorishiro::edition::ee::tasks::reindex_scheduler::TenantReindexScheduler;
    let first = task
        .run_at(&ctx, &loco_rs::task::Vars::default(), at(0))
        .await;
    assert!(first.is_err());
    assert_eq!(dispatcher.attempts.load(Ordering::SeqCst), 2);

    let schedule =
        yorishiro::models::_entities::tenant_reindex_schedules::Entity::find_by_id(tenant.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
    let expected: chrono::DateTime<chrono::FixedOffset> = at(86_400).into();
    assert_eq!(schedule.scheduled_for.unwrap(), expected);

    let second = task
        .run_at(&ctx, &loco_rs::task::Vars::default(), at(0))
        .await;
    assert!(second.is_ok());
    assert_eq!(dispatcher.attempts.load(Ordering::SeqCst), 2);
    let ownership = yorishiro::edition::ee::db::acquire_scheduler_ownership(
        &ctx,
        "yorishiro:tenant-reindex-scheduler",
    )
    .await
    .unwrap()
    .unwrap();
    ownership.release().await.unwrap();
}

/// A migrated SQLite database with one tenant, which is all `set` and the ticker need.
async fn scheduler_context() -> (loco_rs::app::AppContext, Uuid, tempfile::TempDir) {
    use loco_rs::app::Hooks;
    use migration::{Migrator, MigratorTrait};
    use sea_orm::{ActiveModelTrait, ActiveValue, Database};

    let dir = tempfile::tempdir().unwrap();
    let uri = format!(
        "sqlite://{}?mode=rwc",
        dir.path().join("scheduler.sqlite").display()
    );
    yorishiro::db::register_sqlite_extensions();
    let db = Database::connect(&uri).await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    let mut config = yorishiro::App::load_config(&loco_rs::environment::Environment::Any(
        "test_sqlite".into(),
    ))
    .await
    .unwrap();
    config.database.uri = uri;
    let ctx =
        loco_rs::app::AppContext::builder(loco_rs::environment::Environment::Test, db, config)
            .build();
    let tenant = yorishiro::models::_entities::tenant_tenants::ActiveModel {
        name: ActiveValue::Set("interval-test".into()),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .unwrap();
    (ctx, tenant.id, dir)
}

#[tokio::test]
async fn intervals_that_cannot_be_scheduled_are_refused() {
    use yorishiro::edition::ee::tasks::reindex_scheduler::set;

    let (ctx, tenant_id, _dir) = scheduler_context().await;
    for bad in [
        "", "abc", "1D", "P", "PT", "P1Y", "P1M", "P0D", "P1.5D", "PT1M", "PT299S",
    ] {
        assert!(
            set(&ctx.db, tenant_id, bad, None).await.is_err(),
            "{bad:?} must be refused"
        );
    }
    for good in ["P1D", "P1W", "PT6H", "P1DT12H", "PT5M", "PT300S"] {
        set(&ctx.db, tenant_id, good, None)
            .await
            .unwrap_or_else(|error| panic!("{good:?} must be accepted: {error:?}"));
    }
}

/// The first run is one interval after the schedule is set, and every tick schedules the next run one interval after itself.
#[tokio::test]
async fn a_schedule_advances_by_its_own_interval() {
    use yorishiro::edition::ee::tasks::reindex_scheduler::{TenantReindexScheduler, get, set};

    let (ctx, tenant_id, _dir) = scheduler_context().await;
    let before = Utc::now();
    set(&ctx.db, tenant_id, "PT6H", None).await.unwrap();
    let first = get(&ctx.db, tenant_id)
        .await
        .unwrap()
        .unwrap()
        .scheduled_for
        .unwrap();
    let six_hours = chrono::Duration::hours(6);
    assert!(first >= before + six_hours);
    assert!(first <= Utc::now() + six_hours);

    let tick = first.to_utc();
    TenantReindexScheduler
        .run_at(&ctx, &loco_rs::task::Vars::default(), tick)
        .await
        .unwrap();
    let next = get(&ctx.db, tenant_id)
        .await
        .unwrap()
        .unwrap()
        .scheduled_for
        .unwrap();
    assert_eq!(next.to_utc(), tick + six_hours);

    set(&ctx.db, tenant_id, "P1W", None).await.unwrap();
    let weekly = get(&ctx.db, tenant_id)
        .await
        .unwrap()
        .unwrap()
        .scheduled_for
        .unwrap();
    assert!(weekly >= before + chrono::Duration::weeks(1));
}
