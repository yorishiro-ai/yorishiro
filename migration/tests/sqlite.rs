//! Every migration must produce a valid schema on SQLite (single-tenant, no RLS/roles/vector/trgm — see `helpers.rs`), not just on Postgres.
use migration::{Migrator, MigratorTrait};
use sea_orm_migration::sea_orm::Database;

#[tokio::test]
async fn all_migrations_apply_to_a_fresh_sqlite_file() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir.path().join("yorishiro.sqlite3");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());

    let db = Database::connect(&url)
        .await
        .expect("connect to sqlite file");

    Migrator::up(&db, None).await.expect("run all migrations");
}

#[tokio::test]
async fn all_migrations_roll_back_and_reapply_on_sqlite() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir.path().join("yorishiro.sqlite3");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());

    let db = Database::connect(&url)
        .await
        .expect("connect to sqlite file");

    Migrator::up(&db, None).await.expect("run all migrations");
    // Exercises every down()'s pg_only/sqlite_only guards (content_schemas' DROP TRIGGER/FUNCTION and its own trigger drop, both authenticate_api_key files), not just up()'s.
    Migrator::down(&db, None)
        .await
        .expect("roll back all migrations");
    Migrator::up(&db, None)
        .await
        .expect("reapply all migrations after rollback");
}
