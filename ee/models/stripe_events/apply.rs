//! Transactional application of verified billing events.

use chrono::{DateTime, Utc};
use loco_rs::app::AppContext;
use sea_orm::ConnectionTrait;

use crate::db;
use crate::edition::ee::data::{plan::Plan, stripe::StripeConfig};
use crate::edition::ee::dtos::stripe::VerifiedStripeEvent;
use crate::edition::ee::models::{stripe_events, tenant_billing};
use crate::error::{ResultExt, YorishiroError};

fn unlinked_customer() -> YorishiroError {
    YorishiroError::BackendUnavailable {
        message: "subscription customer is not linked yet; retry after checkout completes".into(),
    }
}

/// The tenant a subscription event's `customer` field resolves to, or `None` when the object has no `customer` or that customer is not linked yet.
async fn resolve_tenant_by_customer(
    conn: &impl ConnectionTrait,
    object: &serde_json::Value,
) -> Result<Option<uuid::Uuid>, YorishiroError> {
    let Some(customer_id) = object.get("customer").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    Ok(tenant_billing::get_by_stripe_customer(conn, customer_id)
        .await?
        .map(|record| record.tenant_id))
}

/// Applies a verified Stripe event to the tenant model.
///
/// The checkout session that starts a subscription carries the tenant id as `client_reference_id`.
/// `link_stripe_customer` records it so later subscription events, keyed only by Stripe customer id, resolve to the tenant.
///
/// Idempotency and ordering are enforced inside one transaction.
/// A duplicate delivery, or one older than an event already applied for the same customer, is accepted so Stripe stops retrying, but it is not applied again.
///
/// Everything runs in one `DatabaseTransaction`, so the advisory lock releases on commit or rollback.
pub(crate) async fn apply_stripe_event(
    ctx: &AppContext,
    config: &StripeConfig,
    event: VerifiedStripeEvent,
) -> Result<(), YorishiroError> {
    let txn = db::begin_write(&ctx.db).await.internal()?;

    if stripe_events::is_event_processed(&txn, &event.id).await? {
        tracing::info!(
            event_id = event.id,
            "ignoring already-processed Stripe event"
        );
        return Ok(());
    }

    let Some(created) = DateTime::<Utc>::from_timestamp(event.created, 0) else {
        tracing::warn!(
            event_id = event.id,
            created = event.created,
            "ignoring a Stripe event with an unrepresentable `created` timestamp"
        );
        return Ok(());
    };
    // Only subscription events are ordered per customer.
    // `checkout.session.completed` is a one-time link event outside that stream.
    let customer_id = matches!(
        event.event_type.as_str(),
        "customer.subscription.created"
            | "customer.subscription.updated"
            | "customer.subscription.deleted"
    )
    .then_some(&event.data.object)
    .and_then(|object| object.get("customer"))
    .and_then(|value| value.as_str())
    .map(str::to_owned);

    // Serializes concurrent deliveries for one customer, so the second sees the first's write only after it commits.
    if let Some(customer_id) = customer_id.as_deref() {
        db::lock_for_update(&txn, &format!("stripe-customer:{customer_id}"))
            .await
            .internal()?;
    }

    if let Some(customer_id) = customer_id.as_deref()
        && stripe_events::is_stale_for_customer(&txn, customer_id, created).await?
    {
        tracing::info!(
            event_id = event.id,
            customer_id,
            "ignoring a Stripe event older than the last one applied for this customer"
        );
        return Ok(());
    }

    match event.event_type.as_str() {
        "checkout.session.completed" => {
            let object = &event.data.object;
            let (Some(tenant_id), Some(customer_id)) = (
                object
                    .get("client_reference_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| uuid::Uuid::parse_str(s).ok()),
                object.get("customer").and_then(|v| v.as_str()),
            ) else {
                tracing::warn!("checkout.session.completed missing client_reference_id/customer");
                return Ok(());
            };
            tenant_billing::link_stripe_customer(&txn, tenant_id, customer_id).await?;
        }
        "customer.subscription.created" | "customer.subscription.updated" => {
            let object = &event.data.object;
            let Some(tenant_id) = resolve_tenant_by_customer(&txn, object).await? else {
                tracing::warn!(
                    ?customer_id,
                    "subscription event for an unlinked Stripe customer"
                );
                return Err(unlinked_customer());
            };
            let price_id = object
                .pointer("/items/data/0/price/id")
                .and_then(|v| v.as_str());
            let Some(plan) =
                price_id.and_then(|id| Plan::from_stripe_price_id(id, &config.price_mapping))
            else {
                tracing::warn!(
                    ?customer_id,
                    ?price_id,
                    "subscription event with an unmapped price id"
                );
                return Ok(());
            };
            tenant_billing::set_plan(&txn, tenant_id, plan.as_str()).await?;
        }
        "customer.subscription.deleted" => {
            let object = &event.data.object;
            let Some(tenant_id) = resolve_tenant_by_customer(&txn, object).await? else {
                return Err(unlinked_customer());
            };
            tenant_billing::set_plan(&txn, tenant_id, Plan::Free.as_str()).await?;
        }
        _ => {}
    }

    stripe_events::record_processed_event(
        &txn,
        &event.id,
        &event.event_type,
        customer_id.as_deref(),
        created,
    )
    .await?;

    txn.commit().await.internal()
}
