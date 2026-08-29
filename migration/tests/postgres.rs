//! The same apply/roll-back/reapply guarantees as `sqlite.rs`, on PostgreSQL.
//!
//! These exist because a rollback bug reached review that SQLite could not have caught: `identity_workspaces.schema_id` and `content_schemas` reference each other, and the constraint that closes that circle is a separate `ALTER TABLE` on PostgreSQL only.
//! SQLite declares the same foreign key inline in its own `CREATE TABLE`, so dropping the table carries it away and the rollback there succeeded while PostgreSQL's failed with `cannot drop table content_schemas because other objects depend on it`.
//!
//! Skipped when `DATABASE_URL` names no PostgreSQL server, so `cargo test` still works on a machine with no database.
//!
//! Both tests are `#[serial]` because each gets its own database but they share a cluster, and `up()` creates the `yorishiro_app` **role**, which is a cluster-wide object.
//! Run in parallel against a cluster where that role does not exist yet, both reach `CREATE ROLE` at once and one fails with `duplicate key value violates unique constraint "pg_authid_rolname_index"` — the migration's own `EXCEPTION WHEN duplicate_object` catches a role that already existed, not two transactions creating it simultaneously.
//! This passed locally and failed in CI for exactly that reason: the local cluster already had the role from earlier runs, so the race had nothing to lose.
use migration::{Migrator, MigratorTrait};
use sea_orm_migration::sea_orm::{ConnectionTrait, Database};
use serial_test::serial;

/// A throwaway database, dropped and recreated so each run starts from nothing.
///
/// Returns `None` when `DATABASE_URL` is unset or is not PostgreSQL, which is how these tests skip rather than fail on a machine without one.
async fn scratch_db(name: &str) -> Option<sea_orm_migration::sea_orm::DatabaseConnection> {
    let base = std::env::var("DATABASE_URL").ok()?;
    if !base.starts_with("postgres://") {
        return None;
    }

    // Swap only the path, so a password or host containing the database's name is not rewritten.
    let (prefix, _) = base.rsplit_once('/')?;
    let admin_url = format!("{prefix}/postgres");
    let target_url = format!("{prefix}/{name}");

    let admin = Database::connect(&admin_url).await.ok()?;
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

    Some(
        Database::connect(&target_url)
            .await
            .expect("connect to scratch database"),
    )
}

#[tokio::test]
#[serial]
async fn all_migrations_apply_to_a_fresh_postgres_database() {
    let Some(db) = scratch_db("yorishiro_migtest_up").await else {
        return;
    };
    Migrator::up(&db, None).await.expect("run all migrations");
}

/// The gate the rollback bug slipped past: `down()` has to drop the circular foreign key before the tables it ties together, and only PostgreSQL has that constraint as a separate object.
#[tokio::test]
#[serial]
async fn all_migrations_roll_back_and_reapply_on_postgres() {
    let Some(db) = scratch_db("yorishiro_migtest_cycle").await else {
        return;
    };

    Migrator::up(&db, None).await.expect("run all migrations");
    Migrator::down(&db, None)
        .await
        .expect("roll every migration back");
    Migrator::up(&db, None)
        .await
        .expect("reapply after rollback");
}
