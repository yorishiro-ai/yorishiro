//! The webhook's replay and ordering guards, over this crate's own
//! `identity_stripe_processed_events` table.
//! Ported from master's `ee/crates/yorishiro-hosted/src/models/stripe_events.rs`.
//!
//! Stripe retries a delivery on a slow or failed response and does not guarantee ordering, so the
//! same event can arrive twice and an older one can arrive after a newer one. These three
//! functions are what makes applying an event idempotent and monotonic; the controller decides
//! what an event *means*, and asks here whether it should be applied at all.

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
use yorishiro_core::error::{ResultExt, YorishiroError};

/// Whether `event_id` has already been applied.
/// Stripe retries a webhook delivery on a slow or failed response, so the same event can arrive
/// more than once; `event_id` is the primary key of `identity_stripe_processed_events`, so this
/// is a plain existence check.
pub async fn is_event_processed(
    conn: &impl ConnectionTrait,
    event_id: &str,
) -> Result<bool, YorishiroError> {
    #[derive(FromQueryResult)]
    struct Exists {
        exists: bool,
    }

    let row = Exists::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT EXISTS(SELECT 1 FROM identity_stripe_processed_events WHERE event_id = $1) \
         AS exists",
        [event_id.into()],
    ))
    .one(conn)
    .await
    .internal()?;
    Ok(row.is_some_and(|r| r.exists))
}

/// Whether `created` is older than the most recently applied event's `created` for the same
/// `customer_id`. Stripe does not guarantee delivery order, so a delayed/retried delivery of a
/// stale event must not be allowed to undo a newer one that already landed for that customer.
pub async fn is_stale_for_customer(
    conn: &impl ConnectionTrait,
    customer_id: &str,
    created: DateTime<Utc>,
) -> Result<bool, YorishiroError> {
    #[derive(FromQueryResult)]
    struct Latest {
        stripe_created: DateTime<Utc>,
    }

    let row = Latest::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT stripe_created FROM identity_stripe_processed_events \
         WHERE customer_id = $1 ORDER BY stripe_created DESC LIMIT 1",
        [customer_id.into()],
    ))
    .one(conn)
    .await
    .internal()?;
    Ok(row.is_some_and(|r| created < r.stripe_created))
}

/// Records that `event_id` has been applied, so a later retry or reorder of the same or an older
/// event for `customer_id` is rejected by [`is_event_processed`]/[`is_stale_for_customer`].
///
/// `ON CONFLICT (event_id) DO NOTHING`: the caller's [`is_event_processed`] check and this
/// insert are not wrapped in a shared transaction, so two truly concurrent deliveries of the
/// same brand-new event id can both pass that check and both dispatch (harmless, the handlers
/// are idempotent). Without the conflict clause, the loser's insert would then fail on the
/// `event_id` primary key and surface as a spurious `500`.
///
/// `event_type` is written but never read back by any query here, which makes it look
/// removable. It isn't: this table is the billing audit trail, and dropping the column would
/// leave an investigator able to see *that* an event was applied but not *what it did*.
pub async fn record_processed_event(
    conn: &impl ConnectionTrait,
    event_id: &str,
    event_type: &str,
    customer_id: Option<&str>,
    created: DateTime<Utc>,
) -> Result<(), YorishiroError> {
    conn.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO identity_stripe_processed_events \
         (event_id, event_type, customer_id, stripe_created) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (event_id) DO NOTHING",
        [
            event_id.into(),
            event_type.into(),
            customer_id.into(),
            created.into(),
        ],
    ))
    .await
    .internal()?;
    Ok(())
}
