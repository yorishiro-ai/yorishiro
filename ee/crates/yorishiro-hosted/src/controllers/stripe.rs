//! The single public HTTP entry point for Stripe webhooks.
//! Ported from master's `ee/crates/yorishiro-hosted/src/http/controllers/stripe.rs`.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use chrono::{DateTime, Utc};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::{ConnectionTrait, TransactionTrait};
use serde::Deserialize;
use yorishiro_core::db;
use yorishiro_core::error::ResultExt;
use yorishiro_core::error::YorishiroError;
use yorishiro_core::models::tenancy;

use crate::models::{billing, stripe_events};
use crate::services::plan::{Plan, StripePriceMapping};
use crate::services::{hmac_sign, non_empty_env};

/// How far a webhook's `t=` timestamp may drift from now before it's rejected as a possible
/// replay. Stripe's own guidance uses 5 minutes.
const SIGNATURE_TOLERANCE_SECS: i64 = 300;

/// Configuration for the Stripe integration.
/// Both fields are absent by default: a deployment with no `YORISHIRO_STRIPE_WEBHOOK_SECRET` set
/// gets a 501 from the webhook endpoint instead of silently accepting unverifiable requests.
#[derive(Debug, Clone, Default)]
pub struct StripeConfig {
    pub webhook_secret: Option<String>,
    pub price_mapping: StripePriceMapping,
}

impl StripeConfig {
    pub fn from_env() -> Self {
        Self {
            webhook_secret: non_empty_env("YORISHIRO_STRIPE_WEBHOOK_SECRET"),
            price_mapping: StripePriceMapping::from_env(),
        }
    }
}

/// Parses Stripe's `Stripe-Signature` header (`t=<unix ts>,v1=<hex hmac>[,v1=<hex hmac>...]`),
/// checks the timestamp is within tolerance, and verifies at least one `v1` candidate matches
/// the HMAC-SHA256 of `"{timestamp}.{body}"` computed with the webhook secret.
/// Both checks are required: the timestamp check alone doesn't authenticate anything, and the
/// signature check alone doesn't prevent a captured request from being replayed indefinitely.
fn verify_stripe_signature(
    payload: &[u8],
    signature_header: &str,
    secret: &str,
) -> Result<(), &'static str> {
    let mut timestamp: Option<i64> = None;
    let mut candidates = Vec::new();
    for part in signature_header.split(',') {
        let mut kv = part.splitn(2, '=');
        match (kv.next(), kv.next()) {
            (Some("t"), Some(v)) => timestamp = v.parse().ok(),
            (Some("v1"), Some(v)) => candidates.push(v),
            _ => {}
        }
    }
    let timestamp = timestamp.ok_or("missing timestamp in Stripe-Signature header")?;
    if (Utc::now().timestamp() - timestamp).abs() > SIGNATURE_TOLERANCE_SECS {
        return Err("Stripe-Signature timestamp is outside the allowed tolerance");
    }
    if candidates.is_empty() {
        return Err("missing v1 signature in Stripe-Signature header");
    }

    let mut signed_payload = format!("{timestamp}.").into_bytes();
    signed_payload.extend_from_slice(payload);

    if candidates
        .iter()
        .any(|candidate| hmac_sign::verify(secret.as_bytes(), &signed_payload, candidate))
    {
        return Ok(());
    }
    Err("no v1 signature matched the computed HMAC")
}

#[derive(Debug, Deserialize)]
struct StripeEvent {
    id: String,
    #[serde(rename = "type")]
    event_type: String,
    /// Unix timestamp of when Stripe created this event: used to detect a delayed/retried
    /// delivery that arrives after a newer event for the same customer has already landed.
    created: i64,
    data: StripeEventData,
}

#[derive(Debug, Deserialize)]
struct StripeEventData {
    object: serde_json::Value,
}

/// Returns 501 without a configured secret, 400 on a missing/invalid signature or malformed
/// body, and 200 once the event has been applied (or was simply not one we act on).
///
/// Intentionally returns `impl IntoResponse` with raw status codes rather than going through
/// `ApiError`: Stripe expects plain-text error bodies from webhooks, not the JSON
/// `{"error": {...}}` envelope the rest of this API uses.
async fn stripe_webhook(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let config = StripeConfig::from_env();
    let Some(secret) = config.webhook_secret.as_deref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "Stripe billing is not configured on this deployment (set \
             YORISHIRO_STRIPE_WEBHOOK_SECRET to enable it)",
        )
            .into_response();
    };

    let Some(signature_header) = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
    else {
        return (StatusCode::BAD_REQUEST, "missing Stripe-Signature header").into_response();
    };

    if let Err(reason) = verify_stripe_signature(&body, signature_header, secret) {
        tracing::warn!(
            reason,
            "rejected Stripe webhook: signature verification failed"
        );
        return (StatusCode::BAD_REQUEST, reason).into_response();
    }

    let event: StripeEvent = match serde_json::from_slice(&body) {
        Ok(event) => event,
        Err(err) => {
            tracing::warn!(error = %err, "rejected Stripe webhook: invalid JSON body");
            return (StatusCode::BAD_REQUEST, "invalid JSON body").into_response();
        }
    };

    match apply_stripe_event(&ctx, &config, event).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(err) => {
            tracing::error!(error = %err, "failed to process Stripe webhook event");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The tenant a subscription event's `customer` field resolves to, or `None` (logged by the
/// caller as appropriate) when the object has no `customer` field or that customer isn't linked
/// to any tenant yet.
async fn resolve_tenant_by_customer(
    conn: &impl ConnectionTrait,
    object: &serde_json::Value,
) -> Result<Option<uuid::Uuid>, YorishiroError> {
    let Some(customer_id) = object.get("customer").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    Ok(billing::get_by_stripe_customer(conn, customer_id)
        .await?
        .map(|record| record.tenant_id))
}

/// Applies a verified Stripe event to the tenant model.
/// Only the handful of event types needed to keep a tenant's plan/cap in sync are handled;
/// anything else (e.g. invoice events used only for record-keeping on Stripe's side) is accepted
/// but ignored.
///
/// The Stripe object's linkage to a tenant is intentionally simple for this skeleton: the
/// checkout session that starts a subscription is expected to have been created with
/// `client_reference_id` set to the tenant id, which is recorded (`link_stripe_customer`) so
/// later subscription events (keyed only by Stripe customer id) can be traced back to it.
///
/// Idempotency and ordering are enforced inside one transaction (see
/// `identity_stripe_processed_events`, `is_event_processed`/`is_stale_for_customer`): a
/// duplicate delivery of an event already applied, or a delayed delivery of an event older than
/// one already applied for the same customer, is accepted (so Stripe doesn't retry it forever)
/// but not re-applied.
///
/// Everything here runs in one `DatabaseTransaction`, not on `ctx.db` directly: `db::SessionLock`
/// (a held connection, separate from the pool everything else draws from) deadlocked under a
/// small `max_connections` (`config/test.yaml`'s default of 1), since the lock held the pool's
/// only connection while every subsequent query waited for one. It also protected nothing: two
/// concurrent deliveries would still write on different connections outside any shared
/// transaction. `db::lock_for_update(&txn, ...)` is what `POST /setup` already uses for the same
/// shape (a transaction-scoped advisory lock, releasing on commit/rollback, no separate
/// connection to leak) — see the checklist's own #191 note on `SessionLock`'s "held connection
/// only" scope.
async fn apply_stripe_event(
    ctx: &AppContext,
    config: &StripeConfig,
    event: StripeEvent,
) -> Result<(), YorishiroError> {
    let txn = ctx.db.begin().await.internal()?;

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
    // `checkout.session.completed` also carries a `customer` field, but it's a one-time link
    // event with no ordering relationship to the subscription stream: recording it here would
    // set a staleness floor that can reject a `customer.subscription.created` for the same
    // purchase if it happens to arrive first with an earlier `created` (Stripe does not
    // guarantee delivery order between the two).
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

    // Serializes concurrent deliveries for the same customer, inside the transaction that also
    // does the ordering check and the writes: the second caller's `is_stale_for_customer` re-read
    // sees the first caller's write only after it commits. Events with no customer (the one-time
    // checkout link) need no ordering and take no lock.
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
            billing::link_stripe_customer(&txn, tenant_id, customer_id).await?;
        }
        "customer.subscription.created" | "customer.subscription.updated" => {
            let object = &event.data.object;
            let Some(tenant_id) = resolve_tenant_by_customer(&txn, object).await? else {
                tracing::warn!(
                    ?customer_id,
                    "subscription event for an unlinked Stripe customer"
                );
                return Ok(());
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
            let caps = plan.caps();
            billing::set_plan(&txn, tenant_id, plan.as_str()).await?;
            tenancy::set_tenant_max_workspaces(&txn, tenant_id, caps.max_workspaces).await?;
        }
        "customer.subscription.deleted" => {
            let object = &event.data.object;
            let Some(tenant_id) = resolve_tenant_by_customer(&txn, object).await? else {
                return Ok(());
            };
            let caps = Plan::Free.caps();
            billing::set_plan(&txn, tenant_id, Plan::Free.as_str()).await?;
            tenancy::set_tenant_max_workspaces(&txn, tenant_id, caps.max_workspaces).await?;
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

pub fn routes() -> Routes {
    Routes::new()
        .prefix("hosted")
        .add("/stripe/webhook", post(stripe_webhook))
}
