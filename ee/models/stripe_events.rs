//! The webhook's replay and ordering guards, over this crate's own `stripe_events` table.
//!
//! Stripe retries deliveries and does not guarantee ordering, so the same event can arrive twice and an older one can arrive after a newer one.
//! These functions make applying an event idempotent and monotonic, and the controller asks here whether an event should be applied at all.

use sea_orm::SelectExt;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::stripe_events::{ActiveModel, Column, Entity};
use chrono::{DateTime, Utc};
use sea_orm::{ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder};

/// Whether `event_id` has already been applied.
///
/// `event_id` is the primary key of `stripe_events`, so this is a plain existence check.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn is_event_processed(
    conn: &impl ConnectionTrait,
    event_id: &str,
) -> Result<bool, YorishiroError> {
    Entity::find()
        .filter(Column::EventId.eq(event_id))
        .exists(conn)
        .await
        .internal()
}

/// Whether `created` is older than the most recently applied event's `created` for the same `customer_id`.
///
/// A delayed delivery of a stale event must not undo a newer one that already landed for that customer.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn is_stale_for_customer(
    conn: &impl ConnectionTrait,
    customer_id: &str,
    created: DateTime<Utc>,
) -> Result<bool, YorishiroError> {
    let latest = Entity::find()
        .filter(Column::CustomerId.eq(customer_id))
        .order_by_desc(Column::StripeCreated)
        .one(conn)
        .await
        .internal()?;
    Ok(latest.is_some_and(|row| created < row.stripe_created))
}

/// Records that `event_id` has been applied, so a later retry or older event for `customer_id` is rejected by [`is_event_processed`] and [`is_stale_for_customer`].
///
/// `ON CONFLICT (event_id) DO NOTHING` tolerates duplicate inserts, and event application owns the surrounding transaction and customer lock.
///
/// `event_type` is never read back here, but it stays because this table is the billing audit trail.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn record_processed_event(
    conn: &impl ConnectionTrait,
    event_id: &str,
    event_type: &str,
    customer_id: Option<&str>,
    created: DateTime<Utc>,
) -> Result<(), YorishiroError> {
    let active = ActiveModel {
        event_id: ActiveValue::Set(event_id.to_string()),
        event_type: ActiveValue::Set(event_type.to_string()),
        customer_id: ActiveValue::Set(customer_id.map(str::to_string)),
        stripe_created: ActiveValue::Set(created.into()),
        ..Default::default()
    };
    // `Entity::insert` skips model hooks, so run `before_save` for the timestamp.
    let active = sea_orm::ActiveModelBehavior::before_save(active, conn, true)
        .await
        .internal()?;
    Entity::insert(active)
        .on_conflict_do_nothing()
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

mod apply;
pub(crate) use apply::apply_stripe_event;
