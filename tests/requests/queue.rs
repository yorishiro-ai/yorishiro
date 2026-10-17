//! Covers queue enqueue, priority ordering, worker tags, and provider boundaries.
//!
//! The isolated SQLite-provider cases use separate temporary application and queue files, preserving the permitted SQLite/SQLite pair.
//!
//! `tests/workers/queue_routing.rs` covers tag filtering through the public
//! `Queue::run(tags)` API for both SQL providers.  The Valkey provider is covered
//! by `uv run scripts/test_topology.py sqlite-valkey` against a real Valkey service.

use futures::FutureExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use loco_rs::app::Hooks;
use loco_rs::bgworker::{self, BackgroundWorker, Queue, sqlt};
use loco_rs::boot::{self, BootResult};
use loco_rs::config::{PostgresQueueConfig, QueueConfig, RedisQueueConfig, SqliteQueueConfig};
use loco_rs::environment::Environment;
use loco_rs::prelude::async_trait;
use loco_rs::prelude::*;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::edition::ee::controllers::middleware::edition::{LicenceClaims, LicenceState};
use yorishiro::models::_entities::{
    queue_job_lifecycles, tenant_billing, tenant_tenants, workspace_workspaces,
};
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::workers::embedding_sync::{self, EmbeddingSyncArgs, WorkerClass};
use yorishiro::workers::reindex::{self, ReindexArgs};

use crate::requests::close_app_pools_sqlite;

const COMPETING_TAG: &str = "queue-test:competing";
static COMPETING_ORDER: std::sync::OnceLock<Mutex<Vec<(String, String)>>> =
    std::sync::OnceLock::new();
static COMPETING_PROGRESS: std::sync::OnceLock<tokio::sync::watch::Sender<HashMap<String, usize>>> =
    std::sync::OnceLock::new();

#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct CompetingArgs {
    run_id: String,
    name: String,
}

struct CompetingWorker;

#[async_trait]
impl BackgroundWorker<CompetingArgs> for CompetingWorker {
    fn build(_ctx: &AppContext) -> Self {
        Self
    }

    fn tags() -> Vec<String> {
        vec![COMPETING_TAG.to_owned()]
    }

    async fn perform(&self, args: CompetingArgs) -> loco_rs::Result<()> {
        COMPETING_ORDER
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .expect("competing order lock")
            .push((args.run_id.clone(), args.name));
        if let Some(progress) = COMPETING_PROGRESS.get() {
            let mut counts = progress.borrow().clone();
            *counts.entry(args.run_id).or_default() += 1;
            let _ = progress.send(counts);
        }
        Ok(())
    }
}

async fn assert_competing_priority_order(queue: Arc<Queue>) {
    let run_id = Uuid::now_v7().to_string();
    queue
        .register(CompetingWorker)
        .await
        .expect("register competing worker");
    queue
        .enqueue(
            CompetingWorker::class_name(),
            None,
            CompetingArgs {
                run_id: run_id.clone(),
                name: "low".to_owned(),
            },
            Some(vec![COMPETING_TAG.to_owned()]),
            Some(100),
        )
        .await
        .expect("enqueue low priority job");
    queue
        .enqueue(
            CompetingWorker::class_name(),
            None,
            CompetingArgs {
                run_id: run_id.clone(),
                name: "high".to_owned(),
            },
            Some(vec![COMPETING_TAG.to_owned()]),
            Some(300),
        )
        .await
        .expect("enqueue high priority job");

    finish_competing_order(queue, &run_id, vec!["high", "low"]).await;
}

async fn finish_competing_order(queue: Arc<Queue>, run_id: &str, expected: Vec<&str>) {
    let progress = COMPETING_PROGRESS.get_or_init(|| {
        let (sender, _receiver) = tokio::sync::watch::channel(HashMap::new());
        sender
    });
    let mut counts = progress.borrow().clone();
    counts.insert(run_id.to_owned(), 0);
    let _ = progress.send(counts);
    let mut progress = progress.subscribe();
    let running_queue = queue.clone();
    let handle =
        tokio::spawn(async move { running_queue.run(vec![COMPETING_TAG.to_owned()]).await });
    let result = std::panic::AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while progress.borrow().get(run_id).copied().unwrap_or_default() < 2 {
                progress.changed().await.expect("competing worker progress");
            }
        })
        .await
        .expect("competing jobs completed");
    })
    .catch_unwind()
    .await;
    let _ = queue.shutdown();
    let join_result = handle.await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    join_result
        .expect("join competing worker")
        .expect("run competing worker");
    let actual = COMPETING_ORDER
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .expect("competing order lock")
        .iter()
        .filter(|(existing_run, _)| existing_run == run_id)
        .map(|(_, name)| name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        expected.into_iter().map(str::to_owned).collect::<Vec<_>>()
    );
}

async fn with_queue_cleanup<F, Fut>(queue: Arc<Queue>, test: F)
where
    F: FnOnce(Arc<Queue>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let result = std::panic::AssertUnwindSafe(test(queue.clone()))
        .catch_unwind()
        .await;
    let _ = queue.shutdown();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

async fn remove_owned_redis_jobs(uri: &str, queue_name: &str, ids: &[String]) {
    let client = loco_rs::bgworker::redis::RedisPool::open(uri).expect("open owned-key client");
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("connect owned-key client");
    for id in ids {
        let _: usize = redis::cmd("ZREM")
            .arg(format!("queue:{queue_name}"))
            .arg(id)
            .query_async(&mut conn)
            .await
            .expect("remove owned queued member");
        let _: usize = redis::cmd("SREM")
            .arg(format!("processing:{queue_name}"))
            .arg(id)
            .query_async(&mut conn)
            .await
            .expect("remove owned processing member");
        let _: usize = redis::cmd("DEL")
            .arg(format!("job:{id}"))
            .query_async(&mut conn)
            .await
            .expect("remove owned job key");
    }
}

async fn enqueue_owned_redis_job(
    queue: &Queue,
    queue_name: &str,
    args: CompetingArgs,
    tags: Vec<String>,
    priority: i32,
) -> String {
    queue
        .enqueue(
            CompetingWorker::class_name(),
            Some(queue_name.to_owned()),
            args,
            Some(tags),
            Some(priority),
        )
        .await
        .expect("enqueue owned Redis job")
        .expect("Redis job ID")
}

/// Boots a permitted SQLite/SQLite pair with separate temporary files and returns a pool onto the queue file.
async fn with_sqlite_queue<F, Fut>(test: F)
where
    F: FnOnce(AppContext, sea_orm::sqlx::SqlitePool, Uuid) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let dir = tempfile::tempdir().expect("tempdir");
    let queue_path = dir.path().join("queue.sqlite3");
    let queue_uri = format!("sqlite://{}?mode=rwc", queue_path.display());

    // Use Option so we can explicitly clean up even on panic.
    let mut boot: Option<BootResult> = None;
    let mut queue_pool: Option<sea_orm::sqlx::SqlitePool> = None;

    let result = std::panic::AssertUnwindSafe(async {
        let database_uri = format!(
            "sqlite://{}?mode=rwc",
            dir.path().join("app.sqlite3").display()
        );
        let mut config = crate::sqlite_fixture_config(&database_uri);
        config.queue = Some(QueueConfig::Sqlite(SqliteQueueConfig {
            uri: queue_uri.clone(),
            dangerously_flush: false,
            enable_logging: false,
            max_connections: 2,
            min_connections: 1,
            connect_timeout: 5000,
            idle_timeout: 5000,
            poll_interval_sec: 1,
            num_workers: 1,
            reaper: None,
        }));

        let boot_res = App::boot(boot::StartMode::ServerOnly, &Environment::Test, config)
            .await
            .expect("boot with a sqlite queue");
        boot = Some(boot_res);
        let boot_res = boot.as_ref().expect("boot result");
        if let Some(relay) = boot_res
            .app_context
            .shared_store
            .get::<tokio::task::AbortHandle>()
        {
            relay.abort();
        }

        let pool = sea_orm::sqlx::SqlitePool::connect(&queue_uri)
            .await
            .expect("connect to the queue file");
        queue_pool = Some(pool.clone());

        let tenant = tenant_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("queue-test-tenant".into()),
            ..Default::default()
        }
        .insert(&boot_res.app_context.db)
        .await
        .expect("insert queue test tenant");
        let workspace = workspace_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("queue-test-workspace".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_owned()),
            ..Default::default()
        }
        .insert(&boot_res.app_context.db)
        .await
        .expect("insert queue test workspace");
        test(boot_res.app_context.clone(), pool.clone(), workspace.id).await;
    })
    .catch_unwind()
    .await;

    // Close connections before the temporary database and queue directory is removed.
    if let Some(b) = &boot {
        if let Some(ref queue) = b.app_context.queue_provider {
            let _ = queue.shutdown();
        }
        close_app_pools_sqlite(
            &b.app_context,
            dir.path().join("app.sqlite3").to_str().unwrap(),
        )
        .await;
    }
    if let Some(pool) = queue_pool {
        pool.close().await;
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

fn args_for(class: WorkerClass, workspace_id: Uuid) -> EmbeddingSyncArgs {
    EmbeddingSyncArgs {
        lifecycle_id: None,
        dispatch_id: None,
        publishing_epoch: None,
        workspace_id,
        entity_id: Uuid::now_v7(),
        worker_class: class,
    }
}

async fn run_dispatch_relay(ctx: &AppContext) {
    yorishiro::workers::dispatch::recover_pending_on(ctx, &ctx.db).await;
}

/// A job enqueued through `enqueue_for_class` reaches the configured queue.
#[tokio::test]
#[serial_test::serial(process_environment)]
async fn enqueue_for_class_puts_a_row_in_the_queue() {
    if !crate::require_sqlite_backend() {
        return;
    }
    with_sqlite_queue(|ctx, pool, workspace_id| async move {
        embedding_sync::enqueue_for_class(&ctx, args_for(WorkerClass::Shared, workspace_id))
            .await
            .expect("enqueue");
        assert!(
            sqlt::get_jobs(&pool, None, None)
                .await
                .expect("queue remains untouched before the relay runs")
                .is_empty(),
            "the producer records durable intent but does not publish"
        );
        run_dispatch_relay(&ctx).await;

        let jobs = sqlt::get_jobs(&pool, None, None).await.expect("get_jobs");
        assert_eq!(jobs.len(), 1, "jobs: {jobs:?}");
        assert_eq!(jobs[0].name, "EmbeddingSyncWorker");
    })
    .await;
}

#[tokio::test]
#[serial_test::serial(process_environment)]
async fn accepted_enqueue_with_failed_provider_id_record_keeps_the_outbox_row() {
    if !crate::require_sqlite_backend() {
        return;
    }
    with_sqlite_queue(|ctx, pool, workspace_id| async move {
        ctx.db
            .execute_unprepared(
                "CREATE TRIGGER fail_provider_job_id BEFORE UPDATE OF provider_job_id ON queue_job_lifecycles \
                 WHEN NEW.provider_job_id IS NOT NULL BEGIN \
                 SELECT RAISE(FAIL, 'test provider ID write failure'); END",
            )
            .await
            .expect("install temporary provider ID trigger");

        embedding_sync::enqueue_for_class(&ctx, args_for(WorkerClass::Shared, workspace_id))
            .await
            .expect("dispatch reports the provider acceptance");
        assert!(
            sqlt::get_jobs(&pool, None, None)
                .await
                .expect("queue remains untouched before the relay runs")
                .is_empty(),
            "the producer does not publish directly"
        );
        run_dispatch_relay(&ctx).await;

        let jobs = sqlt::get_jobs(&pool, None, None).await.expect("read queue");
        assert_eq!(jobs.len(), 1, "provider accepted one queue message");
        let lifecycle = queue_job_lifecycles::Entity::find()
            .one(&ctx.db)
            .await
            .expect("read lifecycle")
            .expect("lifecycle was recorded before enqueue");
        assert_eq!(lifecycle.status, "queued");
        assert_eq!(lifecycle.provider_job_id, None);
        let outbox = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(
            lifecycle.id,
        )
        .one(&ctx.db)
        .await
        .expect("read outbox")
        .expect("outbox remains after failed correlation write");
        assert!(
            yorishiro::models::queue_job_dispatch_outbox::due(&ctx.db)
                .await
                .expect("read cooldown-filtered outbox")
                .is_empty(),
            "the dispatch cooldown temporarily hides the accepted job"
        );
        let mut outbox = outbox.into_active_model();
        outbox.last_attempt_at = sea_orm::ActiveValue::Set(Some(
            Utc.timestamp_opt(1_000, 0).unwrap().fixed_offset(),
        ));
        outbox.update(&ctx.db).await.expect("age attempt timestamp");

        let due = yorishiro::models::queue_job_dispatch_outbox::due(&ctx.db)
            .await
            .expect("read recovered outbox");
        assert_eq!(due.len(), 1, "the accepted dispatch becomes eligible again");
        assert_eq!(due[0].lifecycle_id, Some(lifecycle.id));
        assert_eq!(due[0].payload, jobs[0].data);
    })
    .await;
}

/// Each `WorkerClass` enqueues under its own worker type and carries that class's tag.
///
/// `tags()` is a per-type static, so the class has to select the worker type at `perform_later` time rather than travel in the job's arguments; getting that backwards is the easy mistake here.
/// A regression that sent every class to one worker, or dropped the tag, shows up here as two rows sharing a name or carrying `None`.
#[tokio::test]
#[serial_test::serial(process_environment)]
async fn each_worker_class_is_sent_to_its_own_target() {
    if !crate::require_sqlite_backend() {
        return;
    }
    with_sqlite_queue(|ctx, pool, workspace_id| async move {
        let tenant_id = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("read the workspace")
            .expect("the workspace exists")
            .tenant_id;
        for class in [
            WorkerClass::Shared,
            WorkerClass::Official,
            WorkerClass::TenantPrivate,
        ] {
            embedding_sync::enqueue_for_class(&ctx, args_for(class, workspace_id))
                .await
                .expect("enqueue");
        }
        assert!(
            sqlt::get_jobs(&pool, None, None)
                .await
                .expect("read queue before relay")
                .is_empty(),
            "producers do not publish worker jobs"
        );
        run_dispatch_relay(&ctx).await;

        let jobs = sqlt::get_jobs(&pool, None, None).await.expect("get_jobs");
        assert_eq!(jobs.len(), 3, "jobs: {jobs:?}");

        let mut seen: Vec<(String, Vec<String>, i32)> = jobs
            .iter()
            .map(|job| {
                (
                    job.name.clone(),
                    job.tags.clone().unwrap_or_default(),
                    job.priority,
                )
            })
            .collect();
        seen.sort();

        assert_eq!(
            seen,
            vec![
                (
                    "EmbeddingSyncWorker".to_string(),
                    vec!["worker-target:official".to_string()],
                    200,
                ),
                (
                    "EmbeddingSyncWorker".to_string(),
                    vec!["worker-target:shared".to_string()],
                    100,
                ),
                (
                    "EmbeddingSyncWorker".to_string(),
                    vec![format!("worker-target:tenant:{tenant_id}")],
                    300,
                ),
            ]
        );
    })
    .await;
}

#[tokio::test]
#[serial_test::serial(process_environment)]
async fn each_reindex_worker_class_is_sent_to_its_own_target() {
    if !crate::require_sqlite_backend() {
        return;
    }
    with_sqlite_queue(|ctx, pool, workspace_id| async move {
        let tenant_id = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("read the workspace")
            .expect("the workspace exists")
            .tenant_id;
        for class in [
            WorkerClass::Shared,
            WorkerClass::Official,
            WorkerClass::TenantPrivate,
        ] {
            reindex::enqueue_for_class(
                &ctx,
                ReindexArgs {
                    lifecycle_id: None,
                    dispatch_id: None,
                    publishing_epoch: None,
                    workspace_id,
                    worker_class: class,
                    startup: false,
                },
            )
            .await
            .expect("enqueue");
        }
        assert!(
            sqlt::get_jobs(&pool, None, None)
                .await
                .expect("read queue before relay")
                .is_empty(),
            "reindex producers do not publish worker jobs"
        );
        run_dispatch_relay(&ctx).await;

        let jobs = sqlt::get_jobs(&pool, None, None).await.expect("get_jobs");
        assert_eq!(jobs.len(), 3, "jobs: {jobs:?}");

        let mut seen: Vec<(String, Vec<String>, i32)> = jobs
            .iter()
            .map(|job| {
                (
                    job.name.clone(),
                    job.tags.clone().unwrap_or_default(),
                    job.priority,
                )
            })
            .collect();
        seen.sort();

        assert_eq!(
            seen,
            vec![
                (
                    "ReindexWorker".to_string(),
                    vec!["worker-target:official".to_string()],
                    200,
                ),
                (
                    "ReindexWorker".to_string(),
                    vec!["worker-target:shared".to_string()],
                    100,
                ),
                (
                    "ReindexWorker".to_string(),
                    vec![format!("worker-target:tenant:{tenant_id}")],
                    300,
                ),
            ]
        );
    })
    .await;
}

/// Reindex uses the queue policy; embedding admission is process-local and records no queue limit.
#[tokio::test]
#[serial_test::serial(process_environment)]
async fn missing_billing_uses_free_policy_for_reindex_but_not_embedding() {
    if !crate::require_sqlite_backend() {
        return;
    }
    with_sqlite_queue(|ctx, _pool, workspace_id| async move {
        embedding_sync::enqueue_for_class(&ctx, args_for(WorkerClass::Official, workspace_id))
            .await
            .expect("embedding enqueue without billing");
        reindex::enqueue_for_class(
            &ctx,
            ReindexArgs {
                lifecycle_id: None,
                dispatch_id: None,
                publishing_epoch: None,
                workspace_id,
                worker_class: WorkerClass::Official,
                startup: false,
            },
        )
        .await
        .expect("reindex enqueue without billing");

        let rows = queue_job_lifecycles::Entity::find()
            .all(&ctx.db)
            .await
            .expect("read queue lifecycles");
        assert_eq!(rows.len(), 2);
        let embedding = rows
            .iter()
            .find(|row| row.job_name == "embedding_sync")
            .unwrap();
        assert_eq!(embedding.plan, None);
        assert_eq!(embedding.concurrency_key, None);
        assert_eq!(embedding.concurrency_limit, None);
        let reindex = rows.iter().find(|row| row.job_name == "reindex").unwrap();
        assert_eq!(reindex.plan.as_deref(), Some("free"));
        assert_eq!(
            reindex.concurrency_key.as_deref(),
            Some("official:official")
        );
        assert_eq!(reindex.concurrency_limit, None);
    })
    .await;
}

/// A concrete licence inserted after boot must replace the default state used by queue policy.
#[tokio::test]
#[serial_test::serial(process_environment)]
async fn active_licence_plan_takes_precedence_over_billing_for_dispatch() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let guard = crate::EnvGuard::capture(&["YORISHIRO_PLAN_POLICY_ENABLED"]);
    guard.set("YORISHIRO_PLAN_POLICY_ENABLED", "true");
    with_sqlite_queue(|ctx, _pool, workspace_id| async move {
        ctx.shared_store
            .insert(Arc::new(LicenceState::licensed(LicenceClaims {
                sub: "queue-test".into(),
                plan: "pro".into(),
                exp: chrono::Utc::now().timestamp() + 3600,
            })));

        let workspace = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("find workspace")
            .expect("workspace");
        tenant_billing::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(workspace.tenant_id),
            plan: sea_orm::ActiveValue::Set(Some("free".into())),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert billing");

        reindex::enqueue_for_class(
            &ctx,
            ReindexArgs {
                lifecycle_id: None,
                dispatch_id: None,
                publishing_epoch: None,
                workspace_id,
                worker_class: WorkerClass::Official,
                startup: false,
            },
        )
        .await
        .expect("reindex enqueue with active licence");
        let row = queue_job_lifecycles::Entity::find()
            .one(&ctx.db)
            .await
            .expect("read queue lifecycle")
            .expect("queue lifecycle");
        assert_eq!(row.plan.as_deref(), Some("pro"));
        assert_eq!(row.concurrency_key.as_deref(), Some("official:official"));
        assert_eq!(row.concurrency_limit, Some(4));
    })
    .await;
}

#[tokio::test]
#[serial_test::serial(process_environment)]
async fn enterprise_billing_plan_uses_team_queue_policy() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let guard = crate::EnvGuard::capture(&[
        "YORISHIRO_PLAN_POLICY_ENABLED",
        "YORISHIRO_WORKER_CLASS_SHARED_LIMIT",
        "YORISHIRO_WORKER_CLASS_TENANT_PRIVATE_LIMIT",
        "YORISHIRO_WORKER_OFFICIAL_LIMIT",
    ]);
    guard.set("YORISHIRO_WORKER_CLASS_SHARED_LIMIT", "2");
    guard.set("YORISHIRO_PLAN_POLICY_ENABLED", "true");
    guard.set("YORISHIRO_WORKER_CLASS_TENANT_PRIVATE_LIMIT", "3");
    guard.set("YORISHIRO_WORKER_OFFICIAL_LIMIT", "9");
    with_sqlite_queue(|ctx, _pool, workspace_id| async move {
        let workspace = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("find workspace")
            .expect("workspace");
        tenant_billing::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(workspace.tenant_id),
            plan: sea_orm::ActiveValue::Set(Some("enterprise".into())),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert enterprise billing");

        for (class, key, limit) in [
            (WorkerClass::Shared, "shared:shared", 2),
            (WorkerClass::TenantPrivate, "tenant_private:tenant:", 3),
            (WorkerClass::Official, "official:official", 9),
        ] {
            reindex::enqueue_for_class(
                &ctx,
                ReindexArgs {
                    lifecycle_id: None,
                    dispatch_id: None,
                    publishing_epoch: None,
                    workspace_id,
                    worker_class: class,
                    startup: false,
                },
            )
            .await
            .expect("reindex enqueue with enterprise billing");
            let row = queue_job_lifecycles::Entity::find()
                .filter(queue_job_lifecycles::Column::WorkerClass.eq(class.as_db_str()))
                .one(&ctx.db)
                .await
                .expect("read queue lifecycle")
                .expect("queue lifecycle");
            assert_eq!(row.plan.as_deref(), Some("team"));
            assert!(row.concurrency_key.as_deref().unwrap().starts_with(key));
            assert_eq!(row.concurrency_limit, Some(limit));
        }
    })
    .await;
}

#[tokio::test]
#[serial_test::serial(process_environment)]
async fn enterprise_billing_plan_uses_team_queue_fallback_without_official_override() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let guard = crate::EnvGuard::capture(&[
        "YORISHIRO_PLAN_POLICY_ENABLED",
        "YORISHIRO_WORKER_OFFICIAL_LIMIT",
    ]);
    guard.remove("YORISHIRO_WORKER_OFFICIAL_LIMIT");
    guard.remove("YORISHIRO_PLAN_POLICY_ENABLED");
    with_sqlite_queue(|ctx, _pool, workspace_id| async move {
        let workspace = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("find workspace")
            .expect("workspace");
        tenant_billing::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(workspace.tenant_id),
            plan: sea_orm::ActiveValue::Set(Some("enterprise".into())),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert enterprise billing");

        reindex::enqueue_for_class(
            &ctx,
            ReindexArgs {
                lifecycle_id: None,
                dispatch_id: None,
                publishing_epoch: None,
                workspace_id,
                worker_class: WorkerClass::Official,
                startup: false,
            },
        )
        .await
        .expect("reindex enqueue with enterprise billing");
        let row = queue_job_lifecycles::Entity::find()
            .one(&ctx.db)
            .await
            .expect("read queue lifecycle")
            .expect("queue lifecycle");
        assert_eq!(row.plan.as_deref(), Some("team"));
        assert_eq!(row.concurrency_key.as_deref(), Some("official:official"));
        assert_eq!(row.concurrency_limit, None);
    })
    .await;
}

#[tokio::test]
#[serial_test::serial(process_environment)]
async fn pro_plan_has_no_official_cap_when_policy_is_disabled() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let guard = crate::EnvGuard::capture(&[
        "YORISHIRO_PLAN_POLICY_ENABLED",
        "YORISHIRO_WORKER_OFFICIAL_LIMIT",
    ]);
    guard.remove("YORISHIRO_PLAN_POLICY_ENABLED");
    guard.remove("YORISHIRO_WORKER_OFFICIAL_LIMIT");
    with_sqlite_queue(|ctx, _pool, workspace_id| async move {
        let workspace = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        tenant_billing::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(workspace.tenant_id),
            plan: sea_orm::ActiveValue::Set(Some("pro".into())),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        reindex::enqueue_for_class(
            &ctx,
            ReindexArgs {
                lifecycle_id: None,
                dispatch_id: None,
                publishing_epoch: None,
                workspace_id,
                worker_class: WorkerClass::Official,
                startup: false,
            },
        )
        .await
        .unwrap();
        let row = queue_job_lifecycles::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.concurrency_limit, None);
    })
    .await;
}

#[tokio::test]
#[serial_test::serial(process_environment)]
async fn disabled_policy_ignores_official_override_without_changing_metadata() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let guard = crate::EnvGuard::capture(&[
        "YORISHIRO_PLAN_POLICY_ENABLED",
        "YORISHIRO_WORKER_OFFICIAL_LIMIT",
    ]);
    guard.remove("YORISHIRO_PLAN_POLICY_ENABLED");
    guard.set("YORISHIRO_WORKER_OFFICIAL_LIMIT", "9");
    with_sqlite_queue(|ctx, _pool, workspace_id| async move {
        let workspace = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        tenant_billing::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(workspace.tenant_id),
            plan: sea_orm::ActiveValue::Set(Some("enterprise".into())),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        reindex::enqueue_for_class(
            &ctx,
            ReindexArgs {
                lifecycle_id: None,
                dispatch_id: None,
                publishing_epoch: None,
                workspace_id,
                worker_class: WorkerClass::Official,
                startup: false,
            },
        )
        .await
        .unwrap();
        let row = queue_job_lifecycles::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.plan.as_deref(), Some("team"));
        assert_eq!(row.concurrency_key.as_deref(), Some("official:official"));
        assert_eq!(row.concurrency_limit, None);
    })
    .await;
}

#[tokio::test]
#[serial_test::serial(process_environment)]
async fn present_invalid_billing_plan_remains_diagnostic() {
    if !crate::require_sqlite_backend() {
        return;
    }
    with_sqlite_queue(|ctx, _pool, workspace_id| async move {
        let workspace = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("find workspace")
            .expect("workspace");
        tenant_billing::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(workspace.tenant_id),
            plan: sea_orm::ActiveValue::Set(Some("invalid".into())),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert invalid billing");

        let error = reindex::enqueue_for_class(
            &ctx,
            ReindexArgs {
                lifecycle_id: None,
                dispatch_id: None,
                publishing_epoch: None,
                workspace_id,
                worker_class: WorkerClass::Official,
                startup: false,
            },
        )
        .await
        .expect_err("invalid billing must remain diagnostic");
        assert!(error.to_string().contains("unknown plan value"));
    })
    .await;
}

#[tokio::test]
#[serial_test::serial(process_environment)]
async fn invalid_billing_does_not_affect_embedding_process_local_admission() {
    if !crate::require_sqlite_backend() {
        return;
    }
    with_sqlite_queue(|ctx, _pool, workspace_id| async move {
        let workspace = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("find workspace")
            .expect("workspace");
        tenant_billing::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(workspace.tenant_id),
            plan: sea_orm::ActiveValue::Set(Some("invalid".into())),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert invalid billing");

        embedding_sync::enqueue_for_class(&ctx, args_for(WorkerClass::Official, workspace_id))
            .await
            .expect("embedding admission must not consult billing policy");
        let row = queue_job_lifecycles::Entity::find()
            .one(&ctx.db)
            .await
            .expect("read queue lifecycle")
            .expect("queue lifecycle");
        assert_eq!(row.job_name, "embedding_sync");
        assert_eq!(row.concurrency_key, None);
        assert_eq!(row.concurrency_limit, None);
    })
    .await;
}

#[tokio::test]
#[serial_test::serial(queue_backend)]
#[serial_test::serial(process_environment)]
async fn sqlite_background_queue_orders_competing_jobs_without_sleeping() {
    if !super::is_sqlite_queue() {
        return;
    }
    let directory = tempfile::tempdir().expect("queue tempdir");
    let uri = format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("competing.sqlite3").display()
    );
    let config = SqliteQueueConfig {
        uri,
        dangerously_flush: false,
        enable_logging: false,
        max_connections: 2,
        min_connections: 1,
        connect_timeout: 5_000,
        idle_timeout: 5_000,
        poll_interval_sec: 1,
        num_workers: 1,
        reaper: None,
    };
    let provider = bgworker::sqlt::create_provider(&config)
        .await
        .expect("SQLite queue");
    let queue = Arc::new(provider);
    with_queue_cleanup(queue, |queue| async move {
        queue.setup().await.expect("set up SQLite queue");
        assert_competing_priority_order(queue).await;
    })
    .await;
}

#[tokio::test]
#[serial_test::serial(queue_backend)]
#[serial_test::serial(process_environment)]
async fn postgres_background_queue_orders_competing_jobs_without_sleeping() {
    if !super::super::require_postgres_backend() {
        return;
    }
    let config = PostgresQueueConfig {
        uri: std::env::var("DATABASE_URL").expect("PostgreSQL DATABASE_URL"),
        dangerously_flush: false,
        enable_logging: false,
        max_connections: 2,
        min_connections: 1,
        connect_timeout: 5_000,
        idle_timeout: 5_000,
        poll_interval_sec: 1,
        num_workers: 1,
        reaper: None,
    };
    let provider = bgworker::pg::create_provider(&config)
        .await
        .expect("Postgres queue");
    let queue = Arc::new(provider);
    with_queue_cleanup(queue, |queue| async move {
        queue.setup().await.expect("set up Postgres queue");
        assert_competing_priority_order(queue).await;
    })
    .await;
}

#[tokio::test]
#[serial_test::serial(queue_backend)]
#[serial_test::serial(process_environment)]
async fn redis_bounded_scan_is_observable_at_the_queue_boundary() {
    let Some(uri) = crate::valkey_test_url() else {
        eprintln!(
            "skipping Valkey bounded-scan test: the lane's queue is not reserved database 15"
        );
        return;
    };
    let queue_name = format!("bounded-scan:{}", Uuid::now_v7());
    let cleanup_uri = uri.clone();
    let config = RedisQueueConfig {
        uri,
        dangerously_flush: false,
        queues: Some(vec![queue_name.clone()]),
        num_workers: 1,
        reaper: None,
    };
    let queue = Arc::new(
        bgworker::redis::create_provider(&config)
            .await
            .expect("Redis queue"),
    );
    let cleanup_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let cleanup_ids_for_test = cleanup_ids.clone();
    let worker_handle = Arc::new(tokio::sync::Mutex::new(None));
    let worker_handle_for_test = worker_handle.clone();
    let result = std::panic::AssertUnwindSafe(async {
        queue.setup().await.expect("set up Redis queue");
        queue
            .register(CompetingWorker)
            .await
            .expect("register competing worker");
        let run_id = format!("bounded-redis-{}", Uuid::now_v7());
        let control_id = enqueue_owned_redis_job(
            &queue,
            &queue_name,
            CompetingArgs {
                run_id: run_id.clone(),
                name: "in-window-control".to_owned(),
            },
            vec![COMPETING_TAG.to_owned()],
            400,
        )
        .await;
        let mut owned_ids = vec![control_id];
        *cleanup_ids_for_test.lock().expect("owned ID lock") = owned_ids.clone();
        let progress = COMPETING_PROGRESS.get_or_init(|| {
            let (sender, _receiver) = tokio::sync::watch::channel(HashMap::new());
            sender
        });
        let mut counts = progress.borrow().clone();
        counts.insert(run_id.clone(), 0);
        let _ = progress.send(counts);
        let mut progress = progress.subscribe();
        let running_queue = queue.clone();
        let handle =
            tokio::spawn(async move { running_queue.run(vec![COMPETING_TAG.to_owned()]).await });
        *worker_handle_for_test.lock().await = Some(handle);
        let run_result = std::panic::AssertUnwindSafe(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while progress.borrow().get(&run_id).copied().unwrap_or_default() < 1 {
                    progress.changed().await.expect("Redis control progress");
                }
            })
            .await
            .expect("Redis in-window control completed");
            for index in 0..1000 {
                owned_ids.push(
                    enqueue_owned_redis_job(
                        &queue,
                        &queue_name,
                        CompetingArgs {
                            run_id: run_id.clone(),
                            name: format!("mismatch-{index}"),
                        },
                        vec!["queue-test:other".to_owned()],
                        300,
                    )
                    .await,
                );
                *cleanup_ids_for_test.lock().expect("owned ID lock") = owned_ids.clone();
            }
            owned_ids.push(
                enqueue_owned_redis_job(
                    &queue,
                    &queue_name,
                    CompetingArgs {
                        run_id: run_id.clone(),
                        name: "matching-beyond-scan".to_owned(),
                    },
                    vec![COMPETING_TAG.to_owned()],
                    200,
                )
                .await,
            );
            *cleanup_ids_for_test.lock().expect("owned ID lock") = owned_ids.clone();
            assert!(
                tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        if progress.borrow().get(&run_id).copied().unwrap_or_default() > 1 {
                            return;
                        }
                        progress
                            .changed()
                            .await
                            .expect("Redis beyond-scan progress");
                    }
                })
                .await
                .is_err(),
                "matching job outside Redis bounded scan was processed"
            );
        })
        .catch_unwind()
        .await;
        let _ = queue.shutdown();
        run_result
    })
    .catch_unwind()
    .await;
    let _ = queue.shutdown();
    let join_result = if let Some(mut handle) = worker_handle.lock().await.take() {
        match tokio::time::timeout(Duration::from_secs(3), &mut handle).await {
            Ok(result) => Some(result),
            Err(_) => {
                handle.abort();
                Some(handle.await)
            }
        }
    } else {
        None
    };
    let owned_ids = cleanup_ids.lock().expect("owned ID lock").clone();
    remove_owned_redis_jobs(&cleanup_uri, &queue_name, &owned_ids).await;
    match result {
        Ok(Ok(())) => match join_result.expect("worker was started") {
            Ok(Ok(())) => {}
            Ok(Err(error)) => panic!("run Redis worker: {error}"),
            Err(error) => panic!("join Redis worker: {error}"),
        },
        Ok(Err(panic)) | Err(panic) => std::panic::resume_unwind(panic),
    }
}
