use std::sync::Arc;

use migration::{Migrator, MigratorTrait};
use sea_orm::{Database, DatabaseConnection};
use tokio::sync::Barrier;
use yorishiro::models::system_maintenance::{self, MaintenanceMode};
use yorishiro::services::db_load_guard::AUTO_REASON;

async fn sqlite_connections() -> (DatabaseConnection, DatabaseConnection, tempfile::TempDir) {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let path = directory.path().join("maintenance.sqlite3");
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let first = Database::connect(&url)
        .await
        .expect("connect first database");
    Migrator::up(&first, None)
        .await
        .expect("migrate first database");
    let second = Database::connect(&url)
        .await
        .expect("connect second database");
    (first, second, directory)
}

async fn race_transition(
    db: DatabaseConnection,
    barrier: Arc<Barrier>,
    expected: system_maintenance::MaintenanceState,
    mode: MaintenanceMode,
    reason: Option<String>,
) -> bool {
    barrier.wait().await;
    system_maintenance::set_if_current(&db, &expected, mode, 300, reason)
        .await
        .expect("conditional maintenance update")
}

#[tokio::test]
async fn competing_enter_transitions_do_not_overwrite_each_other() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let (first, second, _directory) = sqlite_connections().await;
    let expected = system_maintenance::get(&first)
        .await
        .expect("read initial state");
    let barrier = Arc::new(Barrier::new(2));
    let (read_only, full_lock) = tokio::join!(
        race_transition(
            first,
            barrier.clone(),
            expected.clone(),
            MaintenanceMode::ReadOnly,
            Some(AUTO_REASON.to_string()),
        ),
        race_transition(
            second,
            barrier,
            expected,
            MaintenanceMode::FullLock,
            Some("operator maintenance".to_string()),
        ),
    );
    assert_ne!(read_only, full_lock, "exactly one stale transition may win");
}

#[tokio::test]
async fn competing_exit_and_manual_lock_do_not_clear_each_other() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let (first, second, _directory) = sqlite_connections().await;
    system_maintenance::set(
        &first,
        MaintenanceMode::ReadOnly,
        300,
        Some(AUTO_REASON.to_string()),
    )
    .await
    .expect("seed automatic read-only state");
    let expected = system_maintenance::get(&first)
        .await
        .expect("read automatic state");
    let barrier = Arc::new(Barrier::new(2));
    let (off, full_lock) = tokio::join!(
        race_transition(
            first,
            barrier.clone(),
            expected.clone(),
            MaintenanceMode::Off,
            None,
        ),
        race_transition(
            second,
            barrier,
            expected,
            MaintenanceMode::FullLock,
            Some("operator maintenance".to_string()),
        ),
    );
    assert_ne!(off, full_lock, "exactly one stale transition may win");
}
