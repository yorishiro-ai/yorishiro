//! The same apply/roll-back/reapply guarantees as `sqlite.rs`, on PostgreSQL.
//!
//! These exist because a rollback bug reached review that SQLite could not have caught: `workspace_workspaces.schema_id` and `schema_schemas` reference each other, and the constraint that closes that circle is a separate `ALTER TABLE` on PostgreSQL only.
//! SQLite declares the same foreign key inline in its own `CREATE TABLE`, so dropping the table carries it away and the rollback there succeeded while PostgreSQL's failed with `cannot drop table schema_schemas because other objects depend on it`.
//!
//! Skipped by the shared PostgreSQL backend gate when another backend is active.
//!
//! Both tests are `#[serial(postgres_cluster)]` because each gets its own database but they share a cluster, and `up()` creates the `yorishiro_app` **role**, which is a cluster-wide object.
//! Run in parallel against a cluster where that role does not exist yet, both reach `CREATE ROLE` at once and one fails with `duplicate key value violates unique constraint "pg_authid_rolname_index"` — the migration's own `EXCEPTION WHEN duplicate_object` catches a role that already existed, not two transactions creating it simultaneously.
//! This passed locally and failed in CI for exactly that reason: the local cluster already had the role from earlier runs, so the race had nothing to lose.
use futures::FutureExt;
use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, Statement, TransactionTrait};
use serial_test::serial;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};

static SCRATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Returns the 1-indexed position of the named migration in `Migrator::migrations()`.
/// Panics with a clear message if the migration is missing or renamed.
fn migrations_through(name: &str) -> u32 {
    Migrator::migrations()
        .iter()
        .position(|m| m.name() == name)
        .unwrap_or_else(|| panic!("no migration named {name} in Migrator::migrations()")) as u32
        + 1
}

/// A throwaway database, dropped and recreated so each run starts from nothing.
///
/// A `DATABASE_URL` that is present but unusable panics rather than skipping.
async fn scratch_db(name: &str) -> (sea_orm::DatabaseConnection, String, String) {
    let base = std::env::var("DATABASE_URL").expect("PostgreSQL DATABASE_URL");

    // Split the path from the query, so `?sslmode=require` and friends survive onto both derived
    // URLs. Dropping them silently produced a connection failure that the old `.ok()?` turned into
    // a skip, so an SSL-requiring server reported two passing tests that had not run.
    let (without_query, query) = match base.split_once('?') {
        Some((head, q)) => (head, format!("?{q}")),
        None => (base.as_str(), String::new()),
    };
    // Rewrite only the last path segment, so a password or host containing the database's name is left alone.
    let prefix = without_query
        .rsplit_once('/')
        .expect("DATABASE_URL has no database path segment")
        .0;
    let admin_url = format!("{prefix}/postgres{query}");
    let unique_name = format!(
        "yorishiro_mig_{}_{}_{}",
        std::process::id(),
        SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        name
    );
    let target_url = format!("{prefix}/{unique_name}{query}");

    let admin = Database::connect(&admin_url)
        .await
        .expect("connect to the admin database named by DATABASE_URL");
    for sql in [
        format!("DROP DATABASE IF EXISTS {unique_name}"),
        format!("CREATE DATABASE {unique_name}"),
    ] {
        admin
            .execute_unprepared(&sql)
            .await
            .expect("prepare scratch database");
    }
    drop(admin);

    let db = Database::connect(&target_url)
        .await
        .expect("connect to scratch database");
    (db, unique_name, target_url)
}

async fn close_scratch_db(db: sea_orm::DatabaseConnection, name: &str) -> Result<(), String> {
    let mut errors = Vec::new();
    if let Err(error) = db.close().await {
        errors.push(format!("close scratch database pool: {error}"));
    }
    let base = std::env::var("DATABASE_URL")
        .map_err(|error| format!("read DATABASE_URL for scratch cleanup: {error}"))?;
    let (without_query, query) = match base.split_once('?') {
        Some((head, q)) => (head, format!("?{q}")),
        None => (base.as_str(), String::new()),
    };
    let prefix = without_query
        .rsplit_once('/')
        .ok_or_else(|| "DATABASE_URL has no database path segment".to_owned())?
        .0;
    let admin = Database::connect(format!("{prefix}/postgres{query}"))
        .await
        .map_err(|error| format!("connect to the admin database for scratch cleanup: {error}"))?;
    if let Err(error) = admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS {name}"))
        .await
    {
        errors.push(format!("drop scratch database {name}: {error}"));
    }
    if let Err(error) = admin.close().await {
        errors.push(format!("close scratch admin pool: {error}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub(crate) async fn with_timezone_scratch_db<F>(test: F)
where
    F: for<'a> FnOnce(&'a sea_orm::DatabaseConnection) -> futures::future::BoxFuture<'a, ()> + Send,
{
    with_scratch_db("timezone_state", test).await;
}

async fn with_scratch_db<F>(name: &str, test: F)
where
    F: for<'a> FnOnce(&'a sea_orm::DatabaseConnection) -> futures::future::BoxFuture<'a, ()>,
{
    let (db, database_name, _target_url) = scratch_db(name).await;
    let result = AssertUnwindSafe(test(&db)).catch_unwind().await;
    let cleanup = close_scratch_db(db, &database_name).await;
    match (result, cleanup) {
        (Ok(()), Ok(())) => {}
        (Ok(()), Err(error)) => panic!("scratch database cleanup failed: {error}"),
        (Err(panic), Ok(())) => std::panic::resume_unwind(panic),
        (Err(panic), Err(error)) => {
            eprintln!("scratch database cleanup also failed: {error}");
            std::panic::resume_unwind(panic);
        }
    }
}

async fn privilege(db: &sea_orm::DatabaseConnection, action: &str) -> bool {
    db.query_one_raw(Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        format!("SELECT has_table_privilege('yorishiro_app', 'inference_jobs', '{action}')"),
    ))
    .await
    .expect("query privilege")
    .expect("privilege row")
    .try_get_by_index(0)
    .expect("read privilege")
}

async fn rls_enabled(db: &sea_orm::DatabaseConnection) -> bool {
    db.query_one_raw(Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT relrowsecurity FROM pg_class WHERE oid = 'inference_jobs'::regclass",
    ))
    .await
    .expect("query RLS state")
    .expect("RLS state row")
    .try_get_by_index(0)
    .expect("read RLS state")
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn publishing_state_is_select_only_for_tenant_and_shared_lock_is_callable() {
    if !super::super::require_postgres_backend() {
        return;
    }
    with_scratch_db("publishing_state_privileges", |db| {
        async move {
            Migrator::up(db, None).await.expect("migrations");
            let privileges = db.query_one_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres, "SELECT has_table_privilege('yorishiro_app', 'public.queue_publishing_state', 'UPDATE'), has_function_privilege('yorishiro_app', 'public.queue_publishing_epoch_shared_lock()', 'EXECUTE'), has_function_privilege('yorishiro_app', 'public.queue_publishing_epoch_exclusive_lock()', 'EXECUTE'), EXISTS (SELECT 1 FROM pg_proc p CROSS JOIN LATERAL aclexplode(COALESCE(p.proacl, acldefault('f', p.proowner))) a WHERE p.oid = 'public.queue_publishing_epoch_exclusive_lock()'::regprocedure AND a.grantee = 0 AND a.privilege_type = 'EXECUTE')")).await.expect("check privileges").expect("privilege row");
            assert!(!privileges.try_get_by_index::<bool>(0).expect("publisher update privilege"));
            assert!(privileges.try_get_by_index::<bool>(1).expect("shared lock execute privilege"));
            assert!(!privileges.try_get_by_index::<bool>(2).expect("exclusive lock execute privilege"));
            assert!(!privileges.try_get_by_index::<bool>(3).expect("PUBLIC exclusive execute privilege"));
            let role = db.begin().await.expect("begin role test");
            role.execute_unprepared("SET ROLE yorishiro_app").await.expect("set application role");
            let exclusive_call = role.query_one_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres, "SELECT public.queue_publishing_epoch_exclusive_lock()")).await;
            assert!(exclusive_call.is_err(), "tenant role must not execute exclusive lock");
            role.rollback().await.expect("release denied exclusive lock");
            let txn = db.begin().await.expect("begin lock transaction");
            txn.execute_unprepared("SET ROLE yorishiro_app")
                .await
                .expect("set role");
            let result = txn
                .execute_unprepared(
                    "UPDATE public.queue_publishing_state SET epoch = epoch + 1 WHERE id = 1",
                )
                .await;
            assert!(
                result.is_err(),
                "tenant role must not mutate publisher state"
            );
            txn.rollback().await.expect("rollback denied direct update");

            let shared = db.begin().await.expect("begin shared-lock transaction");
            shared.execute_unprepared("SET ROLE yorishiro_app").await.expect("set application role");
            let _ = shared
                .query_one_raw(Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT public.queue_publishing_epoch_shared_lock()",
                ))
                .await
                .expect("shared-lock function")
                .expect("epoch row");
            let blocked_db = db.clone();
            let blocked = tokio::spawn(async move {
                blocked_db
                    .query_one_raw(Statement::from_string(
                        sea_orm::DatabaseBackend::Postgres,
                        "SELECT public.queue_publishing_epoch_exclusive_lock()",
                    ))
                    .await
            });
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert!(!blocked.is_finished(), "exclusive lock must wait for shared lock");
            shared.execute_unprepared("RESET ROLE")
                .await
                .expect("reset role");
            shared.rollback().await.expect("release shared lock transaction");
            blocked.await.expect("exclusive lock task").expect("exclusive lock query");
        }
        .boxed()
    })
    .await;
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn tenant_role_can_insert_a_workspace_scoped_lifecycle_and_outbox_row() {
    if !super::super::require_postgres_backend() {
        return;
    }
    let (db, database_name, target_url) = scratch_db("tenant_outbox_insert").await;
    let result = AssertUnwindSafe(async {
            Migrator::up(&db, None).await.expect("migrations");
            let lifecycle_privileges = db
                .query_one_raw(Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT has_table_privilege('yorishiro_app', 'public.queue_job_lifecycles', 'SELECT'), has_table_privilege('yorishiro_app', 'public.queue_job_lifecycles', 'INSERT'), has_table_privilege('yorishiro_app', 'public.queue_job_lifecycles', 'UPDATE'), has_table_privilege('yorishiro_app', 'public.queue_job_lifecycles', 'DELETE')",
                ))
                .await
                .expect("check scoped lifecycle privileges")
                .expect("lifecycle privilege row");
            assert!(lifecycle_privileges.try_get_by_index::<bool>(0).unwrap());
            assert!(lifecycle_privileges.try_get_by_index::<bool>(1).unwrap());
            assert!(lifecycle_privileges.try_get_by_index::<bool>(2).unwrap());
            assert!(!lifecycle_privileges.try_get_by_index::<bool>(3).unwrap());
            let lock_function_present: bool = db
                .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT to_regprocedure('public.queue_publishing_epoch_shared_lock()') IS NOT NULL AS present",
            ))
            .await
            .expect("check lock function")
            .expect("lock function row")
                .try_get_by_index(0)
                .expect("read lock function presence");
            assert!(lock_function_present, "publisher lock function was migrated");
            let tenant_id = "11111111-1111-4111-8111-111111111111";
            let workspace_id = "22222222-2222-4222-8222-222222222222";
            db.execute_unprepared(&format!(
                "INSERT INTO tenant_tenants (id, name) VALUES ('{tenant_id}', 'outbox-test'); \
                 INSERT INTO workspace_workspaces (id, tenant_id, name, status) VALUES ('{workspace_id}', '{tenant_id}', 'outbox-test', 'active')"
            ))
            .await
            .expect("seed workspace");

            let tenant_pool = yorishiro::db::TenantDb::connect(
                &target_url,
                2,
                "Asia/Tokyo".to_owned(),
            )
            .await
            .expect("build non-superuser tenant pool");
            let mut scoped = tenant_pool.pool().acquire().await.expect("acquire tenant connection");
            let zone: String = sqlx::query_scalar("SHOW TIME ZONE")
                .fetch_one(scoped.as_mut())
                .await
                .expect("configured timezone on newly connected tenant connection");
            assert_eq!(zone, "Asia/Tokyo");
            drop(scoped);
            let mut released = tenant_pool.pool().acquire().await.expect("reacquire released tenant connection");
            let reset_zone: String = sqlx::query_scalar("SHOW TIME ZONE")
                .fetch_one(released.as_mut())
                .await
                .expect("read timezone after tenant connection reset");
            assert_eq!(reset_zone, "UTC");
            drop(released);
            let tenant = uuid::Uuid::parse_str(tenant_id).unwrap();
            let workspace = uuid::Uuid::parse_str(workspace_id).unwrap();
            let txn = tenant_pool
                .begin_for_workspace(tenant, workspace)
                .await
                .expect("begin tenant transaction");
            let id = uuid::Uuid::now_v7();
            yorishiro::models::queue_job_dispatch_outbox::record_in_transaction(
                &txn,
                yorishiro::models::queue_job_lifecycles::Enqueue {
                    id,
                    job_name: "outbox_insert_probe",
                    worker_class: yorishiro::workers::embedding_sync::WorkerClass::Shared,
                    workspace_id: Some(workspace),
                    plan: None,
                    concurrency_key: None,
                    concurrency_limit: None,
                },
                serde_json::json!({"workspace_id": workspace}),
                "ProbeWorker".into(),
                None,
                None,
            )
            .await
            .expect("tenant-scoped outbox write");
            for (name, sql) in [
                ("cross-workspace insert", "INSERT INTO queue_job_lifecycles (id, job_name, worker_class, workspace_id, status, attempt, enqueue_at, created_at, updated_at) VALUES (gen_random_uuid(), 'probe', 'shared', '33333333-3333-4333-8333-333333333333', 'queued', 1, NOW(), NOW(), NOW())"),
                ("global insert", "INSERT INTO queue_job_lifecycles (id, job_name, worker_class, workspace_id, status, attempt, enqueue_at, created_at, updated_at) VALUES (gen_random_uuid(), 'probe', 'shared', NULL, 'queued', 1, NOW(), NOW(), NOW())"),
                ("delete", "DELETE FROM queue_job_lifecycles"),
            ] {
                txn.execute_unprepared("SAVEPOINT lifecycle_probe")
                    .await
                    .expect("savepoint lifecycle probe");
                let denied = txn.execute_unprepared(sql).await;
                assert!(denied.is_err(), "tenant role must be denied {name}");
                txn.execute_unprepared("ROLLBACK TO SAVEPOINT lifecycle_probe")
                    .await
                    .expect("restore transaction after denied statement");
            }
            let hidden = txn
                .query_one_raw(Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT count(*) AS count FROM queue_job_lifecycles WHERE workspace_id IS NULL OR workspace_id <> '22222222-2222-4222-8222-222222222222'",
                ))
                .await
                .expect("read rows outside workspace")
                .expect("count row")
                .try_get_by_index::<i64>(0)
                .expect("read count");
            assert_eq!(hidden, 0, "tenant role cannot see global or cross-workspace lifecycles");
            txn.commit().await.expect("commit tenant transaction");
        })
        .catch_unwind()
        .await;
    let cleanup = close_scratch_db(db, &database_name).await;
    match (result, cleanup) {
        (Ok(()), Ok(())) => {}
        (Ok(()), Err(error)) => panic!("scratch database cleanup failed: {error}"),
        (Err(panic), Ok(())) => std::panic::resume_unwind(panic),
        (Err(panic), Err(error)) => {
            eprintln!("scratch database cleanup also failed: {error}");
            std::panic::resume_unwind(panic);
        }
    }
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn all_migrations_apply_to_a_fresh_postgres_database() {
    if !super::super::require_postgres_backend() {
        return;
    }
    with_scratch_db("up", |db| {
        async move {
            Migrator::up(db, None).await.expect("run all migrations");
        }
        .boxed()
    })
    .await;
}

/// The gate the rollback bug slipped past: `down()` has to drop the circular foreign key before the tables it ties together, and only PostgreSQL has that constraint as a separate object.
#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn all_migrations_roll_back_and_reapply_on_postgres() {
    if !super::super::require_postgres_backend() {
        return;
    }
    with_scratch_db("cycle", |db| {
        async move {
            Migrator::up(db, None).await.expect("run all migrations");
            Migrator::down(db, None)
                .await
                .expect("roll every migration back");
            Migrator::up(db, None)
                .await
                .expect("reapply after rollback");
        }
        .boxed()
    })
    .await;
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn queue_epoch_upgrade_grants_insert_to_existing_inference_job_installations() {
    if !super::super::require_postgres_backend() {
        return;
    }
    with_scratch_db("queue_epoch_inference_insert", |db| {
        async move {
            let workspace_a = "11111111-1111-4111-8111-111111111111";
            let workspace_b = "22222222-2222-4222-8222-222222222222";
            let tenant = "33333333-3333-4333-8333-333333333333";
            let existing_job = "44444444-4444-4444-8444-444444444444";
            let new_job = "55555555-5555-4555-8555-555555555555";

            let attempt_privilege =
                migrations_through("m20260930_000013_inference_job_attempt_privileges");
            Migrator::up(db, Some(attempt_privilege))
                .await
                .expect("apply through the already-shipped inference privilege migration");
            assert!(privilege(db, "SELECT").await);
            assert!(!privilege(db, "INSERT").await);
            assert!(privilege(db, "UPDATE").await);

            db.execute_unprepared(&format!(
                "INSERT INTO tenant_tenants (id, name) VALUES ('{tenant}', 'queue-epoch-upgrade'); \
                 INSERT INTO workspace_workspaces (id, tenant_id, name, status) VALUES ('{workspace_a}', '{tenant}', 'upgrade-a', 'active'), ('{workspace_b}', '{tenant}', 'upgrade-b', 'active'); \
                 INSERT INTO inference_jobs (id, workspace_id, schema_name, status) VALUES ('{existing_job}', '{workspace_a}', 'existing-schema', 'queued')"
            ))
            .await
            .expect("populate the existing installation before the new migration");

            let denied = db.begin().await.expect("begin pre-upgrade tenant insert");
            denied
                .execute_unprepared("SET ROLE yorishiro_app")
                .await
                .expect("assume tenant role before upgrade");
            denied
                .execute_unprepared(&format!(
                    "SELECT set_config('app.current_workspace', '{workspace_a}', false)"
                ))
                .await
                .expect("scope the pre-upgrade insert");
            assert!(
                denied
                    .execute_unprepared(&format!(
                        "INSERT INTO inference_jobs (id, workspace_id, schema_name, status) VALUES ('{new_job}', '{workspace_a}', 'new-schema', 'queued')"
                    ))
                    .await
                    .is_err(),
                "an installation at migration 013 must not yet have INSERT privilege"
            );
            denied.rollback().await.expect("rollback denied insert");

            Migrator::up(db, None)
                .await
                .expect("upgrade the populated installation through the queue migration");
            assert!(privilege(db, "SELECT").await);
            assert!(privilege(db, "INSERT").await);
            assert!(privilege(db, "UPDATE").await);
            assert!(rls_enabled(db).await);
            let preserved: i64 = db
                .query_one_raw(Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    format!("SELECT COUNT(*) FROM inference_jobs WHERE id = '{existing_job}'"),
                ))
                .await
                .expect("read populated inference jobs")
                .expect("existing job count")
                .try_get_by_index(0)
                .expect("decode existing job count");
            assert_eq!(preserved, 1, "the migration preserves existing jobs");

            let tenant_txn = db.begin().await.expect("begin upgraded tenant write");
            tenant_txn
                .execute_unprepared("SET ROLE yorishiro_app")
                .await
                .expect("assume upgraded tenant role");
            tenant_txn
                .execute_unprepared(&format!(
                    "SELECT set_config('app.current_workspace', '{workspace_a}', false)"
                ))
                .await
                .expect("set upgraded tenant workspace");
            tenant_txn
                .execute_unprepared(&format!(
                    "INSERT INTO inference_jobs (id, workspace_id, schema_name, status) VALUES ('{new_job}', '{workspace_a}', 'new-schema', 'queued')"
                ))
                .await
                .expect("tenant can create an inference job after upgrade");
            let cross_workspace: i64 = tenant_txn
                .query_one_raw(Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    format!("SELECT COUNT(*) FROM inference_jobs WHERE workspace_id = '{workspace_b}'"),
                ))
                .await
                .expect("RLS-scoped cross-workspace query")
                .expect("cross-workspace count")
                .try_get_by_index(0)
                .expect("decode cross-workspace count");
            assert_eq!(cross_workspace, 0, "INSERT grant must not weaken RLS");
            tenant_txn
                .rollback()
                .await
                .expect("rollback upgraded tenant fixture writes");

            // Rolling back the queue migration restores migration 013's INSERT privilege state.
            let total = Migrator::migrations().len() as u32;
            let queue_epoch_pos = migrations_through("m20261015_160234_queue_publishing_epoch");
            let steps_back = total - queue_epoch_pos + 1;
            Migrator::down(db, Some(steps_back))
                .await
                .expect("roll back newer migrations through the queue epoch grant");
            assert!(privilege(db, "SELECT").await);
            assert!(!privilege(db, "INSERT").await);
            assert!(privilege(db, "UPDATE").await);
            assert!(rls_enabled(db).await);
            Migrator::up(db, None)
                .await
                .expect("reapply the queue epoch insert grant");
            assert!(privilege(db, "SELECT").await);
            assert!(privilege(db, "INSERT").await);
            assert!(privilege(db, "UPDATE").await);
            assert!(rls_enabled(db).await);
        }
        .boxed()
    })
    .await;
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn inference_job_rls_isolates_workspace_reads_and_updates() {
    if !super::super::require_postgres_backend() {
        return;
    }
    with_scratch_db("attempt_rls", |db| {
        async move {
            Migrator::up(db, None).await.expect("run all migrations");

            let workspace_a = "11111111-1111-4111-8111-111111111111";
            let workspace_b = "22222222-2222-4222-8222-222222222222";
            let tenant = "33333333-3333-4333-8333-333333333333";
            let job_a = "44444444-4444-4444-8444-444444444444";
            let job_b = "55555555-5555-4555-8555-555555555555";

            for sql in [
        format!("INSERT INTO tenant_tenants (id, name) VALUES ('{tenant}', 'rls-test-tenant')"),
        format!(
            "INSERT INTO workspace_workspaces (id, tenant_id, name, status) VALUES ('{workspace_a}', '{tenant}', 'rls-a', 'active'), ('{workspace_b}', '{tenant}', 'rls-b', 'active')"
        ),
        format!(
            "INSERT INTO inference_jobs (id, workspace_id, schema_name, status) VALUES ('{job_a}', '{workspace_a}', 'schema_a', 'queued'), ('{job_b}', '{workspace_b}', 'schema_b', 'queued')"
        ),
            ] {
                db.execute_unprepared(&sql).await.expect("seed RLS fixture");
            }

            let txn = db.begin().await.expect("begin pinned RLS transaction");
            txn.execute_unprepared("SET ROLE yorishiro_app")
                .await
                .expect("assume tenant role");
            txn.execute_unprepared(&format!(
                "SELECT set_config('app.current_workspace', '{workspace_a}', false)"
            ))
            .await
            .expect("set workspace A session config");

            let assertions = AssertUnwindSafe(async {
                let visible: i64 = txn
            .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT COUNT(*) FROM inference_jobs",
            ))
            .await
            .expect("query visible jobs")
            .expect("visible jobs row")
            .try_get_by_index(0)
            .expect("read visible jobs count");
                assert_eq!(visible, 1);

                let updated = txn
            .execute_unprepared(&format!(
                "UPDATE inference_jobs SET status = 'running' WHERE id = '{job_a}'"
            ))
            .await
            .expect("update own workspace job");
                assert_eq!(updated.rows_affected(), 1);
                let cross_update = txn
            .execute_unprepared(&format!(
                "UPDATE inference_jobs SET status = 'running' WHERE id = '{job_b}'"
            ))
            .await
            .expect("attempt cross-workspace update");
                assert_eq!(cross_update.rows_affected(), 0);

                let cross_visible: i64 = txn
            .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("SELECT COUNT(*) FROM inference_jobs WHERE id = '{job_b}'"),
            ))
            .await
            .expect("query cross-workspace job")
            .expect("cross-workspace count row")
            .try_get_by_index(0)
            .expect("read cross-workspace count");
                assert_eq!(cross_visible, 0);
            })
            .catch_unwind()
            .await;

            txn.execute_unprepared("RESET app.current_workspace")
                .await
                .expect("reset workspace session config");
            txn.execute_unprepared("RESET ROLE")
                .await
                .expect("reset tenant role");
            txn.rollback()
                .await
                .expect("rollback pinned RLS transaction");
            if let Err(panic) = assertions {
                std::panic::resume_unwind(panic);
            }
        }
        .boxed()
    })
    .await;
}
