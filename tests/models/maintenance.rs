use std::sync::Arc;

use sea_orm::{Database, DatabaseConnection};
use tokio::sync::Barrier;
use yorishiro::models::system_maintenance::AUTO_REASON;
use yorishiro::models::system_maintenance::{self, MaintenanceMode};

async fn race_transition(
    db: &DatabaseConnection,
    barrier: Arc<Barrier>,
    expected: system_maintenance::MaintenanceState,
    mode: MaintenanceMode,
    reason: Option<String>,
) -> bool {
    barrier.wait().await;
    system_maintenance::set_if_current(db, &expected, mode, 300, reason)
        .await
        .expect("conditional maintenance update")
}

#[tokio::test]
async fn competing_enter_transitions_do_not_overwrite_each_other() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|_request, ctx| async move {
        let first = ctx.db.clone();
        let second = Database::connect(&ctx.config.database.uri)
            .await
            .expect("connect competing pool");
        let expected = system_maintenance::get(&first)
            .await
            .expect("read initial state");
        let barrier = Arc::new(Barrier::new(2));
        let (read_only, full_lock) = tokio::join!(
            race_transition(
                &first,
                barrier.clone(),
                expected.clone(),
                MaintenanceMode::ReadOnly,
                Some(AUTO_REASON.to_string()),
            ),
            race_transition(
                &second,
                barrier,
                expected,
                MaintenanceMode::FullLock,
                Some("operator maintenance".to_string()),
            ),
        );
        second.close().await.expect("close competing pool");
        assert_ne!(read_only, full_lock, "exactly one stale transition may win");
    })
    .await;
}

#[tokio::test]
async fn competing_exit_and_manual_lock_do_not_clear_each_other() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|_request, ctx| async move {
        let first = ctx.db.clone();
        let second = Database::connect(&ctx.config.database.uri)
            .await
            .expect("connect competing pool");
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
                &first,
                barrier.clone(),
                expected.clone(),
                MaintenanceMode::Off,
                None,
            ),
            race_transition(
                &second,
                barrier,
                expected,
                MaintenanceMode::FullLock,
                Some("operator maintenance".to_string()),
            ),
        );
        second.close().await.expect("close competing pool");
        assert_ne!(off, full_lock, "exactly one stale transition may win");
    })
    .await;
}
