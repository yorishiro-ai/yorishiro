use uuid::Uuid;
use yorishiro::tasks::tenant_reindex_scheduler::TenantReindexScheduler;

use chrono::{TimeZone, Utc};

use yorishiro::edition::ee::models::tenant_reindex_schedules::{
    dispatch_reindex_batch, is_due, scheduler_failure_message,
};

const TICK: i64 = 1_700_000_000;

#[tokio::test]
async fn operator_cli_can_configure_replace_inspect_and_disable_a_tenant_schedule() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let tenant =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let config_dir = tempfile::tempdir().unwrap();
        let mut config = ctx.config.clone();
        config.database.auto_migrate = false;
        std::fs::write(
            config_dir.path().join("development.yaml"),
            serde_yaml::to_string(&config).unwrap(),
        )
        .unwrap();
        let run = |args: &[&str]| {
            let output = tempfile::NamedTempFile::new().unwrap();
            let log = output.reopen().unwrap();
            let mut child = Command::new(env!("CARGO_BIN_EXE_yorishiro"))
                .args([
                    "--environment",
                    "development",
                    "task",
                    "tenant_reindex_schedule",
                ])
                .arg(format!("tenant_id:{}", tenant.tenant_id))
                .args(args)
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .env(
                    crate::env::TestEnv::LocoConfigFolder.name(),
                    config_dir.path(),
                )
                .env(crate::env::TestEnv::LocoEnvironment.name(), "development")
                .stdout(Stdio::from(log.try_clone().unwrap()))
                .stderr(Stdio::from(log))
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(15);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "schedule CLI timed out: {}",
                        std::fs::read_to_string(output.path()).unwrap()
                    );
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            child.wait().unwrap();
            (
                status.success(),
                std::fs::read_to_string(output.path()).unwrap(),
            )
        };
        let inspect = |args: &[&str]| {
            let (success, text) = run(args);
            assert!(success, "{text}");
            text.lines()
                .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .expect("CLI emits the schedule as JSON")
        };
        assert!(inspect(&["action:get"])["schedule"].is_null());
        let created = inspect(&["action:set", "interval:PT6H", "timezone:UTC"]);
        assert_eq!(created["tenant_id"], tenant.tenant_id.to_string());
        assert_eq!(created["schedule"]["interval"], "PT6H");
        assert_eq!(created["schedule"]["timezone"], "UTC");
        assert!(created["schedule"]["scheduled_for"].is_string());
        assert_eq!(inspect(&["action:get"]), created);
        let replaced = inspect(&["action:set", "interval:P1D"]);
        assert_eq!(replaced["schedule"]["interval"], "P1D");
        assert!(replaced["schedule"]["timezone"].is_null());
        for args in [
            ["action:set", "interval:PT1M"],
            ["action:unknown", "interval:P1D"],
        ] {
            let (success, text) = run(&args);
            assert!(!success, "{text}");
            assert_eq!(inspect(&["action:get"]), replaced);
        }
        assert!(inspect(&["action:disable"])["schedule"].is_null());
        assert!(inspect(&["action:get"])["schedule"].is_null());
        assert!(inspect(&["action:disable"])["schedule"].is_null());
        assert!(
            yorishiro::edition::ee::models::tenant_reindex_schedules::get(
                &ctx.db,
                tenant.tenant_id
            )
            .await
            .unwrap()
            .is_none()
        );
    })
    .await;
}

#[tokio::test]
async fn schedules_validate_timezones_with_the_application_parser() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        use yorishiro::edition::ee::models::tenant_reindex_schedules::{get, set};

        let tenant =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        for invalid in ["", "Not/AZone"] {
            assert!(
                set(&ctx.db, tenant.tenant_id, "P1D", Some(invalid))
                    .await
                    .is_err()
            );
        }
        assert!(get(&ctx.db, tenant.tenant_id).await.unwrap().is_none());
        set(&ctx.db, tenant.tenant_id, "P1D", Some(" Europe/London "))
            .await
            .unwrap();
        assert_eq!(
            get(&ctx.db, tenant.tenant_id)
                .await
                .unwrap()
                .unwrap()
                .timezone
                .as_deref(),
            Some("Europe/London")
        );
    })
    .await;
}

fn at(offset: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(TICK + offset, 0).single().unwrap()
}

fn install_active_licence(ctx: &loco_rs::app::AppContext) {
    use std::sync::Arc;
    use yorishiro::edition::ee::controllers::middleware::edition::{LicenceClaims, LicenceState};
    ctx.shared_store
        .insert(Arc::new(LicenceState::licensed(LicenceClaims {
            sub: "scheduler-test".into(),
            plan: "pro".into(),
            exp: Utc::now().timestamp() + 3600,
        })));
}

#[tokio::test]
async fn disabled_tenant_retains_schedule_and_history_without_new_dispatches() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        use yorishiro::edition::ee::models::tenant_reindex_schedules::{get, set};

        install_active_licence(&ctx);
        let tenant =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        set(&ctx.db, tenant.tenant_id, "PT6H", None).await.unwrap();
        let first = get(&ctx.db, tenant.tenant_id).await.unwrap().unwrap();
        TenantReindexScheduler
            .run_at(
                &ctx,
                &Default::default(),
                first.scheduled_for.unwrap().to_utc(),
            )
            .await
            .unwrap();
        let history = workspace_dispatch_ids(&ctx.db, tenant.workspace_id).await.0;
        assert!(
            !history.is_empty(),
            "active tenant must dispatch before disablement"
        );
        let schedule = get(&ctx.db, tenant.tenant_id).await.unwrap().unwrap();
        yorishiro::models::tenant_tenants::retirement::disable_by_operator(
            &ctx.db,
            tenant.tenant_id,
            tenant.tenant_id,
        )
        .await
        .unwrap();
        assert_eq!(
            workspace_dispatch_ids(&ctx.db, tenant.workspace_id).await,
            (history.clone(), vec![])
        );
        TenantReindexScheduler
            .run_at(
                &ctx,
                &Default::default(),
                schedule.scheduled_for.unwrap().to_utc(),
            )
            .await
            .unwrap();
        assert_eq!(
            workspace_dispatch_ids(&ctx.db, tenant.workspace_id).await,
            (history, vec![])
        );
        assert_eq!(
            get(&ctx.db, tenant.tenant_id).await.unwrap().unwrap(),
            schedule
        );
    })
    .await;
}

#[tokio::test]
async fn postgres_tick_waits_for_disable_and_rechecks_tenant_activity() {
    if !crate::require_postgres_backend() {
        return;
    }
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        use sea_orm::{EntityTrait, QuerySelect, TransactionTrait};
        use yorishiro::edition::ee::models::tenant_reindex_schedules::{get, set};

        install_active_licence(&ctx);
        let tenant =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        set(&ctx.db, tenant.tenant_id, "PT6H", None).await.unwrap();
        let schedule = get(&ctx.db, tenant.tenant_id).await.unwrap().unwrap();
        let before = workspace_dispatch_ids(&ctx.db, tenant.workspace_id).await;
        let row_lock = ctx.db.begin().await.unwrap();
        yorishiro::models::tenant_tenants::Entity::find_by_id(tenant.tenant_id)
            .lock_exclusive()
            .one(&row_lock)
            .await
            .unwrap()
            .unwrap();
        let fence = format!("tenant-membership:{}", tenant.tenant_id);
        let disable = yorishiro::models::tenant_tenants::retirement::disable_by_operator(
            &ctx.db,
            tenant.tenant_id,
            tenant.tenant_id,
        );
        let tick_after_disable_locks = async {
            // Disable owns the membership fence but cannot update its tenant until we release the row.
            wait_for_membership_lock(&ctx.db, &fence, true).await;
            let vars = Default::default();
            let tick = TenantReindexScheduler.run_at(
                &ctx,
                &vars,
                schedule.scheduled_for.unwrap().to_utc(),
            );
            let release_disable = async {
                wait_for_membership_lock(&ctx.db, &fence, false).await;
                row_lock.rollback().await.unwrap();
            };
            let (result, ()) = tokio::join!(tick, release_disable);
            result.unwrap();
        };
        let (disabled, ()) = tokio::join!(disable, tick_after_disable_locks);
        disabled.unwrap();
        assert!(
            yorishiro::models::tenant_tenants::Entity::find_by_id(tenant.tenant_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .disabled_at
                .is_some()
        );
        assert_eq!(
            workspace_dispatch_ids(&ctx.db, tenant.workspace_id).await,
            before
        );
        assert_eq!(
            get(&ctx.db, tenant.tenant_id).await.unwrap().unwrap(),
            schedule
        );
    })
    .await;
}

async fn workspace_dispatch_ids(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
) -> (Vec<Uuid>, Vec<Uuid>) {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
    use yorishiro::models::_entities::{queue_job_dispatch_outbox, queue_job_lifecycles};

    let lifecycles = queue_job_lifecycles::Entity::find()
        .filter(queue_job_lifecycles::Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(queue_job_lifecycles::Column::Id)
        .all(db)
        .await
        .unwrap();
    let outbox = queue_job_dispatch_outbox::Entity::find()
        .filter(queue_job_dispatch_outbox::Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(queue_job_dispatch_outbox::Column::Id)
        .all(db)
        .await
        .unwrap();
    (
        lifecycles.into_iter().map(|row| row.id).collect(),
        outbox.into_iter().map(|row| row.id).collect(),
    )
}

async fn wait_for_membership_lock(db: &sea_orm::DatabaseConnection, fence: &str, granted: bool) {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let row = db
                .query_one_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND granted = $2 AND classid::bigint = ((hashtextextended($1, 0) >> 32) & 4294967295) AND objid::bigint = (hashtextextended($1, 0) & 4294967295) AND objsubid = 1) AS observed",
                    [fence.into(), granted.into()],
                ))
                .await
                .unwrap()
                .unwrap();
            if row.try_get::<bool>("", "observed").unwrap() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected membership fence holder or waiter was not observed");
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

#[tokio::test]
async fn dispatch_batch_attempts_every_workspace_and_aggregates_failures() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let dispatches: Vec<_> = (0..3)
            .map(|_| {
                (
                    Uuid::now_v7(),
                    yorishiro::workers::reindex::ReindexArgs {
                        lifecycle_id: None,
                        workspace_id: Uuid::now_v7(),
                        worker_class: yorishiro::workers::embedding_sync::WorkerClass::Shared,
                        startup: false,
                        actor: None,
                    },
                )
            })
            .collect();

        let workspaces: Vec<_> = dispatches
            .iter()
            .map(|(_, args)| args.workspace_id)
            .collect();
        // Missing workspaces exercise the real enqueue failure path without replacing dispatch.
        let errors = dispatch_reindex_batch(&ctx, dispatches, None).await;
        assert_eq!(errors.len(), 3);
        for workspace in workspaces {
            assert!(
                errors
                    .iter()
                    .any(|error| error.starts_with(&format!("workspace {workspace}:")))
            );
        }
    })
    .await;
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
    assert!(message.contains("schedule advancement rolled back"));
    assert!(message.contains("workspace one: enqueue failed"));
    assert!(message.contains("scheduler ownership release also failed: unlock failed"));
}

#[tokio::test]
async fn sqlite_task_rolls_back_schedule_when_dispatch_fails() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
    install_active_licence(&ctx);
    if !crate::require_sqlite_backend() { return; }

    use sea_orm::{ConnectionTrait, TransactionTrait};
    use yorishiro::edition::ee::models::tenant_reindex_schedules::{get, set};

    let crate::requests::fixtures::TestTenant { tenant_id, .. } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
    let transaction = ctx.db.begin().await.unwrap();
    for name in ["second"] {
        yorishiro::models::tenancy::create_workspace(&transaction, tenant_id, name, None, None)
            .await
            .unwrap();
    }
    transaction.commit().await.unwrap();
    // A failed outbox insert must roll back both schedule advancement and all jobs.
    ctx.db.execute_unprepared(
        "CREATE TRIGGER refuse_test_dispatch BEFORE INSERT ON queue_job_dispatch_outbox BEGIN SELECT RAISE(ABORT, 'test outbox unavailable'); END"
    ).await.unwrap();
    set(&ctx.db, tenant_id, "P1D", None).await.unwrap();
    let tick = get(&ctx.db, tenant_id)
        .await
        .unwrap()
        .unwrap()
        .scheduled_for
        .unwrap()
        .to_utc();

    let task = TenantReindexScheduler;
    let first = task
        .run_at(&ctx, &loco_rs::task::Vars::default(), tick)
        .await
        .expect_err("outbox rejects both jobs");
    let diagnostic = first.to_string();
    assert!(diagnostic.contains("2 workspace(s)"), "{diagnostic}");
    assert!(
        diagnostic.contains("test outbox unavailable"),
        "{diagnostic}"
    );

    let schedule = get(&ctx.db, tenant_id).await.unwrap().unwrap();
    let expected: chrono::DateTime<chrono::FixedOffset> = tick.into();
    assert_eq!(schedule.scheduled_for.unwrap(), expected);

    ctx.db.execute_unprepared("DROP TRIGGER refuse_test_dispatch").await.unwrap();
    let second = task
        .run_at(&ctx, &loco_rs::task::Vars::default(), tick)
        .await;
    assert!(second.is_ok());
    assert_eq!(get(&ctx.db, tenant_id).await.unwrap().unwrap().scheduled_for.unwrap().to_utc(), tick + chrono::Duration::days(1));

    let ownership = yorishiro::edition::ee::db::acquire_scheduler_ownership(
        &ctx,
        "yorishiro:tenant-reindex-scheduler",
    )
    .await
    .unwrap()
    .unwrap();
    ownership.release().await.unwrap();
    }).await;
}

#[tokio::test]
async fn intervals_that_cannot_be_scheduled_are_refused() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        use yorishiro::edition::ee::models::tenant_reindex_schedules::set;

        let crate::requests::fixtures::TestTenant { tenant_id, .. } =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        for bad in [
            "",
            "abc",
            "1D",
            "P",
            "PT",
            "P1Y",
            "P1M",
            "P0D",
            "P1.5D",
            "PT1M",
            "PT299S",
            "P1D1D",
            "PT1H1H",
            "PT9223372036854775807S",
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
    })
    .await;
}

/// The first run is one interval after the schedule is set, and every tick schedules the next run one interval after itself.
#[tokio::test]
async fn a_schedule_advances_by_its_own_interval() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        install_active_licence(&ctx);
        use yorishiro::edition::ee::models::tenant_reindex_schedules::{get, set};

        let crate::requests::fixtures::TestTenant { tenant_id, .. } =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
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
    })
    .await;
}

#[tokio::test]
async fn licensed_task_ticks_schedule_while_unlicensed_task_only_scans_mismatches() {
    use std::sync::Arc;
    use yorishiro::edition::ee::controllers::middleware::edition::{LicenceClaims, LicenceState};
    use yorishiro::edition::ee::models::tenant_reindex_schedules::{get, set};

    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        install_active_licence(&ctx);
        let crate::requests::fixtures::TestTenant { tenant_id, .. } =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        set(&ctx.db, tenant_id, "PT6H", None).await.unwrap();
        let before = get(&ctx.db, tenant_id)
            .await
            .unwrap()
            .unwrap()
            .scheduled_for
            .unwrap();

        ctx.shared_store
            .insert(Arc::new(LicenceState::licensed(LicenceClaims {
                sub: "scheduler-test".into(),
                plan: "pro".into(),
                exp: Utc::now().timestamp() + 3600,
            })));
        TenantReindexScheduler
            .run_at(&ctx, &loco_rs::task::Vars::default(), Utc::now())
            .await
            .unwrap();
        let after = get(&ctx.db, tenant_id)
            .await
            .unwrap()
            .unwrap()
            .scheduled_for
            .unwrap();
        assert_eq!(after, before, "not-yet-due schedule stays unchanged");

        ctx.shared_store.insert(Arc::new(LicenceState::default()));
        TenantReindexScheduler
            .run_at(&ctx, &loco_rs::task::Vars::default(), Utc::now())
            .await
            .unwrap();
        let after_unlicensed = get(&ctx.db, tenant_id)
            .await
            .unwrap()
            .unwrap()
            .scheduled_for
            .unwrap();
        assert_eq!(
            after_unlicensed, before,
            "unlicensed tick does not advance schedules"
        );
    })
    .await;
}

/// Invalid stored intervals fail without advancing the schedule.
#[tokio::test]
async fn invalid_stored_intervals_are_rejected_without_advancement() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        install_active_licence(&ctx);
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        use yorishiro::edition::ee::models::tenant_reindex_schedules::{get, set};

        let crate::requests::fixtures::TestTenant { tenant_id, .. } =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        for interval in ["PT1S", "P1D1D", "PT9223372036854775S"] {
            set(&ctx.db, tenant_id, "P1D", None).await.unwrap();
            // Bypass input validation to exercise detection of corrupt stored data.
            yorishiro::models::_entities::tenant_reindex_schedules::Entity::update_many()
                .col_expr(
                    yorishiro::models::_entities::tenant_reindex_schedules::Column::Interval,
                    sea_orm::sea_query::Expr::value(interval),
                )
                .filter(
                    yorishiro::models::_entities::tenant_reindex_schedules::Column::TenantId
                        .eq(tenant_id),
                )
                .exec(&ctx.db)
                .await
                .unwrap();
            let tick = get(&ctx.db, tenant_id)
                .await
                .unwrap()
                .unwrap()
                .scheduled_for
                .unwrap()
                .to_utc();
            TenantReindexScheduler
                .run_at(&ctx, &loco_rs::task::Vars::default(), tick)
                .await
                .expect_err("invalid stored interval must fail");
            let next = get(&ctx.db, tenant_id)
                .await
                .unwrap()
                .unwrap()
                .scheduled_for
                .unwrap()
                .to_utc();
            assert_eq!(next, tick, "{interval}");
        }
    })
    .await;
}
