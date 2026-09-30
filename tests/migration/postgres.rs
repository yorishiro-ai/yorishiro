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

/// A throwaway database, dropped and recreated so each run starts from nothing.
///
/// A `DATABASE_URL` that is present but unusable panics rather than skipping.
async fn scratch_db(name: &str) -> (sea_orm::DatabaseConnection, String) {
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
    (db, unique_name)
}

async fn close_scratch_db(db: sea_orm::DatabaseConnection, name: &str) {
    db.close().await.expect("close scratch database pool");
    let base = std::env::var("DATABASE_URL").expect("PostgreSQL DATABASE_URL");
    let (without_query, query) = match base.split_once('?') {
        Some((head, q)) => (head, format!("?{q}")),
        None => (base.as_str(), String::new()),
    };
    let prefix = without_query
        .rsplit_once('/')
        .expect("DATABASE_URL has no database path segment")
        .0;
    let admin = Database::connect(format!("{prefix}/postgres{query}"))
        .await
        .expect("connect to the admin database for scratch cleanup");
    admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS {name}"))
        .await
        .expect("drop scratch database");
    admin.close().await.expect("close scratch admin pool");
}

async fn with_scratch_db<F>(name: &str, test: F)
where
    F: for<'a> FnOnce(&'a sea_orm::DatabaseConnection) -> futures::future::BoxFuture<'a, ()>,
{
    let (db, database_name) = scratch_db(name).await;
    let result = AssertUnwindSafe(test(&db)).catch_unwind().await;
    close_scratch_db(db, &database_name).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
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
async fn inference_job_attempt_migration_grants_update_to_app_role() {
    if !super::super::require_postgres_backend() {
        return;
    }
    with_scratch_db("attempt_privileges", |db| {
        async move {
            Migrator::up(db, None).await.expect("run all migrations");
            assert!(privilege(db, "SELECT").await);
            assert!(privilege(db, "UPDATE").await);
            Migrator::down(db, Some(1))
                .await
                .expect("roll back the privilege migration");
            assert!(privilege(db, "SELECT").await);
            assert!(!privilege(db, "UPDATE").await);
            assert!(!rls_enabled(db).await);
            Migrator::up(db, None)
                .await
                .expect("reapply the privilege migration");
            assert!(privilege(db, "SELECT").await);
            assert!(privilege(db, "UPDATE").await);
            assert!(rls_enabled(db).await);
        }
        .boxed()
    })
    .await;
}

#[tokio::test]
#[serial(postgres_cluster)]
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
