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
