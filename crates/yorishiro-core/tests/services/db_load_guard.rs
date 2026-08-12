use std::time::Duration;

use sqlx::PgPool;

use crate::repositories::maintenance::MaintenanceMode;
use crate::services::db_load_guard::{AUTO_REASON, active_connections, decide};

const SUSTAIN: Duration = Duration::from_secs(30);
const LONG: Duration = Duration::from_secs(31);
const SHORT: Duration = Duration::from_secs(5);

#[test]
fn drops_to_read_only_only_after_the_load_is_sustained() {
    assert_eq!(
        decide(MaintenanceMode::Off, None, SHORT, Duration::ZERO, SUSTAIN),
        None,
        "a spike is not an outage"
    );
    assert_eq!(
        decide(MaintenanceMode::Off, None, LONG, Duration::ZERO, SUSTAIN),
        Some(MaintenanceMode::ReadOnly)
    );
}

/// The distinction the whole design turns on: an operator who asked for read-only before a
/// restore must not have it lifted because the database went quiet.
#[test]
fn lifts_only_what_it_set_itself() {
    assert_eq!(
        decide(
            MaintenanceMode::ReadOnly,
            Some(AUTO_REASON),
            Duration::ZERO,
            LONG,
            SUSTAIN
        ),
        Some(MaintenanceMode::Off),
        "its own state comes back on its own"
    );
    assert_eq!(
        decide(
            MaintenanceMode::ReadOnly,
            Some("restoring a backup"),
            Duration::ZERO,
            LONG,
            SUSTAIN
        ),
        None,
        "somebody else's read-only stays"
    );
    assert_eq!(
        decide(
            MaintenanceMode::ReadOnly,
            None,
            Duration::ZERO,
            LONG,
            SUSTAIN
        ),
        None,
        "a read-only with no reason is not ours either"
    );
}

/// A full lock is a deliberate act -- a restore, a migration. Load has nothing to say about it.
#[test]
fn never_touches_a_full_lock() {
    assert_eq!(
        decide(
            MaintenanceMode::FullLock,
            Some(AUTO_REASON),
            LONG,
            LONG,
            SUSTAIN
        ),
        None
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn counts_the_connections_it_claims_to(pool: PgPool) {
    // This test's own connection is active while the query runs, so the floor is 1.
    let n = active_connections(&pool).await.unwrap();
    assert!(n >= 1, "at least this query is active, got {n}");
}
