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

/// The tenant a subscription event's `customer` field resolves to, or `None` (logged by the caller as appropriate) when the object has no `customer` field or that customer isn't linked to any tenant yet.
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
/// The checkout session that starts a subscription is expected to have `client_reference_id` set to the tenant id, which is recorded (`link_stripe_customer`) so later subscription events (keyed only by Stripe customer id) can be traced back to it.
///
/// Idempotency and ordering are enforced inside one transaction (see `stripe_events`, `is_event_processed`/`is_stale_for_customer`): a duplicate delivery, or a delayed delivery older than one already applied for the same customer, is accepted (so Stripe doesn't retry it forever) but not re-applied.
///
/// Everything here runs in one `DatabaseTransaction`, not on `ctx.db` directly: `db::lock_for_update(&txn, ...)` is a transaction-scoped advisory lock that releases on commit or rollback with no separate connection to leak.
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
    // Only the subscription events are ordered per customer.
    // `checkout.session.completed` also carries a `customer` field but is a one-time link event with no ordering relationship to the subscription stream, so it's excluded here.
    let customer_id = matches!(
        event.event_type.as_str(),
        "customer.subscription.created"
            | "customer.subscription.updated"
            | "customer.subscription.deleted"
    )
    .then(|| {
        event
            .data
            .object
            .get("customer")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    })
    .flatten();

    // Serializes concurrent deliveries for the same customer: the second caller's `is_stale_for_customer` re-read sees the first caller's write only after it commits.
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
                .get("items")
                .and_then(|v| v.get("data"))
                .and_then(|v| v.get(0))
                .and_then(|v| v.get("price"))
                .and_then(|v| v.get("id"))
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
