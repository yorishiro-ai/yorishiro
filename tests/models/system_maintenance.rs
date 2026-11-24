use std::sync::Arc;

use sea_orm::{Database, DatabaseConnection};
use tokio::sync::Barrier;
use yorishiro::models::system_maintenance::AUTO_REASON;
use yorishiro::models::system_maintenance::{self, MaintenanceMode};

#[test]
fn refusal_preserves_operator_reasons_and_only_blocks_the_intended_operations() {
    for mode in [
        MaintenanceMode::Off,
        MaintenanceMode::ReadOnly,
        MaintenanceMode::FullLock,
    ] {
        for is_write in [false, true] {
            for reason in [None, Some("operator maintenance".to_string())] {
                let state = system_maintenance::MaintenanceState {
                    mode,
                    retry_after: 17,
                    reason: reason.clone(),
                };
                let refusal = state.refusal(is_write);
                if mode == MaintenanceMode::Off || (mode == MaintenanceMode::ReadOnly && !is_write)
                {
                    assert!(refusal.is_none());
                    continue;
                }
                let Some(yorishiro::YorishiroError::Maintenance {
                    message,
                    read_only,
                    retry_after,
                }) = refusal
                else {
                    panic!("maintenance must return a structured refusal");
                };
                assert_eq!(read_only, mode == MaintenanceMode::ReadOnly);
                assert_eq!(retry_after, 17);
                if let Some(reason) = reason {
                    assert_eq!(message, reason);
                } else {
                    assert_eq!(
                        message,
                        if mode == MaintenanceMode::ReadOnly {
                            "the deployment is read-only for maintenance; writes are disabled"
                        } else {
                            "the deployment is unavailable for maintenance"
                        }
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn missing_singleton_defaults_to_off_and_stale_reasons_cannot_override_an_operator() {
    use sea_orm::EntityTrait;

    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let initial = system_maintenance::get(&ctx.db).await.unwrap();
        system_maintenance::set(
            &ctx.db,
            MaintenanceMode::ReadOnly,
            17,
            Some("operator".into()),
        )
        .await
        .unwrap();
        assert!(
            !system_maintenance::set_if_current(&ctx.db, &initial, MaintenanceMode::Off, 1, None,)
                .await
                .unwrap()
        );
        let mut observed = system_maintenance::get(&ctx.db).await.unwrap();
        observed.reason = Some("stale operator reason".into());
        assert!(
            !system_maintenance::set_if_current(&ctx.db, &observed, MaintenanceMode::Off, 1, None,)
                .await
                .unwrap()
        );
        let current = system_maintenance::get(&ctx.db).await.unwrap();
        assert_eq!(current.mode, MaintenanceMode::ReadOnly);
        assert_eq!(current.reason.as_deref(), Some("operator"));
        system_maintenance::Entity::delete_by_id(true)
            .exec(&ctx.db)
            .await
            .unwrap();
        let missing = system_maintenance::get(&ctx.db).await.unwrap();
        assert_eq!(missing.mode, MaintenanceMode::Off);
        assert_eq!(missing.reason, None);
        assert!(missing.retry_after > 0);
    })
    .await;
}

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
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
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
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
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
