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
use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database};
use serial_test::serial;

/// A throwaway database, dropped and recreated so each run starts from nothing.
///
/// A `DATABASE_URL` that is present but unusable panics rather than skipping.
async fn scratch_db(name: &str) -> sea_orm::DatabaseConnection {
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
    let target_url = format!("{prefix}/{name}{query}");

    let admin = Database::connect(&admin_url)
        .await
        .expect("connect to the admin database named by DATABASE_URL");
    for sql in [
        format!("DROP DATABASE IF EXISTS {name}"),
        format!("CREATE DATABASE {name}"),
    ] {
        admin
            .execute_unprepared(&sql)
            .await
            .expect("prepare scratch database");
    }
    drop(admin);

    Database::connect(&target_url)
        .await
        .expect("connect to scratch database")
}

#[tokio::test]
#[serial(postgres_cluster)]
async fn all_migrations_apply_to_a_fresh_postgres_database() {
    if !super::super::require_postgres_backend() {
        return;
    }
    let db = scratch_db("yorishiro_migtest_up").await;
    Migrator::up(&db, None).await.expect("run all migrations");
}

/// The gate the rollback bug slipped past: `down()` has to drop the circular foreign key before the tables it ties together, and only PostgreSQL has that constraint as a separate object.
#[tokio::test]
#[serial(postgres_cluster)]
async fn all_migrations_roll_back_and_reapply_on_postgres() {
    if !super::super::require_postgres_backend() {
        return;
    }
    let db = scratch_db("yorishiro_migtest_cycle").await;

    Migrator::up(&db, None).await.expect("run all migrations");
    Migrator::down(&db, None)
        .await
        .expect("roll every migration back");
    Migrator::up(&db, None)
        .await
        .expect("reapply after rollback");
}
