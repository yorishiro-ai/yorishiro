use sqlx::PgPool;

use crate::models::maintenance::{MaintenanceMode, MaintenanceState, get, set};

fn state(mode: MaintenanceMode) -> MaintenanceState {
    MaintenanceState {
        mode,
        retry_after: 300,
        reason: None,
    }
}

/// The table that decides who is refused.
/// Reads pass in read-only, writes do not, and full lock takes both.
#[test]
fn refusal_follows_the_mode_and_the_request_kind() {
    assert!(state(MaintenanceMode::Off).refusal(false).is_none());
    assert!(state(MaintenanceMode::Off).refusal(true).is_none());

    assert!(state(MaintenanceMode::ReadOnly).refusal(false).is_none());
    assert!(state(MaintenanceMode::ReadOnly).refusal(true).is_some());

    assert!(state(MaintenanceMode::FullLock).refusal(false).is_some());
    assert!(state(MaintenanceMode::FullLock).refusal(true).is_some());
}

/// Read-only is 423 and full lock is 503. The distinction is the point: 423 says the resource is locked while the server is fine, 503 says the server is not serving.
#[test]
fn each_mode_maps_to_its_own_status() {
    let (status, body) = state(MaintenanceMode::ReadOnly)
        .refusal(true)
        .unwrap()
        .into_http_parts();
    assert_eq!(status, 423);
    assert_eq!(body["error"]["retry_after_seconds"], 300);

    let (status, _) = state(MaintenanceMode::FullLock)
        .refusal(false)
        .unwrap()
        .into_http_parts();
    assert_eq!(status, 503);
}

/// An operator's reason reaches the caller; without one the message still says which mode it is, rather than leaving a bare status code to be interpreted.
#[test]
fn the_operators_reason_is_what_the_caller_reads() {
    let with_reason = MaintenanceState {
        mode: MaintenanceMode::FullLock,
        retry_after: 60,
        reason: Some("restoring from backup, back by 09:00".to_string()),
    };
    let (_, body) = with_reason.refusal(false).unwrap().into_http_parts();
    assert_eq!(
        body["error"]["message"],
        "restoring from backup, back by 09:00"
    );

    let (_, body) = state(MaintenanceMode::ReadOnly)
        .refusal(true)
        .unwrap()
        .into_http_parts();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("read-only"),
        "the default message should still say which mode it is: {body}"
    );
}

/// A value this crate does not understand is an error, not "off".
/// Treating a corrupt row as "serve everything" would turn it into an outage of the protection itself.
#[test]
fn an_unknown_stored_mode_is_not_read_as_off() {
    assert_eq!(
        MaintenanceMode::from_db_str("off"),
        Some(MaintenanceMode::Off)
    );
    assert_eq!(
        MaintenanceMode::from_db_str("read_only"),
        Some(MaintenanceMode::ReadOnly)
    );
    assert_eq!(MaintenanceMode::from_db_str("paused"), None);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_fresh_deployment_is_not_in_maintenance(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let current = get(&mut conn).await.unwrap();
    assert_eq!(current.mode, MaintenanceMode::Off);
}

#[sqlx::test(migrations = "../../migrations")]
async fn set_then_get_round_trips(pool: PgPool) {
    set(
        &pool,
        MaintenanceMode::ReadOnly,
        45,
        Some("migrating".to_string()),
    )
    .await
    .unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let current = get(&mut conn).await.unwrap();
    assert_eq!(current.mode, MaintenanceMode::ReadOnly);
    assert_eq!(current.retry_after, 45);
    assert_eq!(current.reason.as_deref(), Some("migrating"));

    // And back off again, clearing the reason with it.
    set(&pool, MaintenanceMode::Off, 300, None).await.unwrap();
    let current = get(&mut conn).await.unwrap();
    assert_eq!(current.mode, MaintenanceMode::Off);
    assert!(current.reason.is_none());
}
