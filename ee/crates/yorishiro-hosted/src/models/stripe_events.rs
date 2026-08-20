//! The webhook's replay and ordering guards, over this repository's own `identity.stripe_processed_events` table.
//!
//! Stripe retries a delivery on a slow or failed response and does not guarantee ordering, so the
//! same event can arrive twice and an older one can arrive after a newer one.
//! These three functions are what makes applying an event idempotent and monotonic; the controller
//! decides what an event *means*, and asks here whether it should be applied at all.
//!
//! Kept beside `billing.rs` rather than in the controller: both own a table this repository adds,
//! and the two halves of one webhook's state have no reason to sit in different layers.

use chrono::{DateTime, Utc};
use sea_query::{Alias, Expr, Iden, OnConflict, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use yorishiro_core::error::{ResultExt, YorishiroError};

#[derive(Iden)]
enum StripeProcessedEvents {
    Table,
    EventId,
    EventType,
    CustomerId,
    StripeCreated,
}

/// Whether `event_id` has already been applied.
/// Stripe retries a webhook delivery on a slow or failed response, so the same event can arrive more than once; `event_id` is the primary key of `identity.stripe_processed_events`, so this is a plain existence check.
pub async fn is_event_processed(pool: &PgPool, event_id: &str) -> Result<bool, YorishiroError> {
    let (sql, values) = Query::select()
        .expr(Expr::val(1))
        .from((Alias::new("identity"), StripeProcessedEvents::Table))
        .and_where(Expr::col(StripeProcessedEvents::EventId).eq(event_id))
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<(i32,)> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?;
    Ok(row.is_some())
}

/// Whether `created` is older than the most recently applied event's `created` for the same `customer_id`.
/// Stripe does not guarantee delivery order, so a delayed/retried delivery of a stale event must not be allowed to undo a newer one that already landed for that customer.
pub async fn is_stale_for_customer(
    pool: &PgPool,
    customer_id: &str,
    created: DateTime<Utc>,
) -> Result<bool, YorishiroError> {
    let (sql, values) = Query::select()
        .column(StripeProcessedEvents::StripeCreated)
        .from((Alias::new("identity"), StripeProcessedEvents::Table))
        .and_where(Expr::col(StripeProcessedEvents::CustomerId).eq(customer_id))
        .order_by(StripeProcessedEvents::StripeCreated, sea_query::Order::Desc)
        .limit(1)
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<(DateTime<Utc>,)> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?;
    Ok(row.is_some_and(|(latest,)| created < latest))
}

/// Records that `event_id` has been applied, so a later retry or reorder of the same or an older event for `customer_id` is rejected by [`is_event_processed`]/[`is_stale_for_customer`].
///
/// `ON CONFLICT (event_id) DO NOTHING`: the caller's [`is_event_processed`] check and this insert are not wrapped in a shared transaction, so two truly concurrent deliveries of the same brand-new event id can both pass that check and both dispatch (harmless, the handlers are idempotent, e.g. `set_tenant_plan` just sets the same plan twice).
/// Without the conflict clause, the loser's insert would then fail on the `event_id` primary key and surface as a spurious `500`; Stripe would retry, and the retry's own `is_event_processed` check would find the winner's already-recorded row and correctly skip re-dispatch.
/// The conflict clause just avoids that unnecessary `500`/retry round trip.
///
/// `event_type` is written but never read back by any query here, which makes it look removable.
/// It isn't.
/// This table is the billing audit trail: when a tenant's plan or cap ends up wrong, the only record of which Stripe event types actually landed (and in what order, via `stripe_created`/`processed_at`) is these rows.
/// Dropping the column would leave an investigator able to see *that* an event was applied but not *what it did*, right when that distinction matters most.
/// It also mirrors the caller's `match event.event_type` dispatch, so keeping it stops the recorded history and the branching logic from drifting apart.
pub async fn record_processed_event(
    pool: &PgPool,
    event_id: &str,
    event_type: &str,
    customer_id: Option<&str>,
    created: DateTime<Utc>,
) -> Result<(), YorishiroError> {
    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), StripeProcessedEvents::Table))
        .columns([
            StripeProcessedEvents::EventId,
            StripeProcessedEvents::EventType,
            StripeProcessedEvents::CustomerId,
            StripeProcessedEvents::StripeCreated,
        ])
        .values_panic([
            event_id.into(),
            event_type.into(),
            customer_id.into(),
            created.into(),
        ])
        .on_conflict(
            OnConflict::column(StripeProcessedEvents::EventId)
                .do_nothing()
                .to_owned(),
        )
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_with(&sql, values)
        .execute(pool)
        .await
        .internal()?;
    Ok(())
}
