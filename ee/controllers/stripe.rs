//! The single public HTTP entry point for Stripe webhooks.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use chrono::{DateTime, Utc};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::{ConnectionTrait, TransactionTrait};

use crate::db;
use crate::ee::data::non_empty_env;
use crate::ee::data::plan::{Plan, StripePriceMapping};
use crate::ee::models::tenant_tenants;
use crate::ee::models::{stripe_events, tenant_billing};
use crate::error::ResultExt;
use crate::error::YorishiroError;

pub mod inbound {
    use super::SIGNATURE_TOLERANCE_SECS;
    use axum::http::HeaderMap;
    use chrono::Utc;
    use serde::Deserialize;

    use crate::ee::controllers::hmac_sign;

    #[derive(Debug)]
    pub enum Error {
        MissingSignatureHeader,
        Signature(&'static str),
        InvalidJson(serde_json::Error),
    }

    #[derive(Debug, Deserialize)]
    pub struct VerifiedStripeEvent {
        pub(super) id: String,
        #[serde(rename = "type")]
        pub(super) event_type: String,
        /// Unix timestamp of when Stripe created this event.
        pub(super) created: i64,
        pub(super) data: StripeEventData,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct StripeEventData {
        pub(super) object: serde_json::Value,
    }

    pub(super) fn verify(
        headers: &HeaderMap,
        payload: &[u8],
        secret: &str,
    ) -> Result<VerifiedStripeEvent, Error> {
        verify_at(headers, payload, secret, Utc::now().timestamp())
    }

    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub fn verify_at(
        headers: &HeaderMap,
        payload: &[u8],
        secret: &str,
        now: i64,
    ) -> Result<VerifiedStripeEvent, Error> {
        let mut signature_headers = headers.get_all("stripe-signature").iter();
        let Some(signature_value) = signature_headers.next() else {
            return Err(Error::MissingSignatureHeader);
        };
        if signature_headers.next().is_some() {
            return Err(Error::Signature("invalid Stripe-Signature header"));
        }
        let Ok(signature_header) = signature_value.to_str() else {
            return Err(Error::MissingSignatureHeader);
        };

        verify_signature(payload, signature_header, secret, now).map_err(Error::Signature)?;
        serde_json::from_slice(payload).map_err(Error::InvalidJson)
    }

    fn verify_signature(
        payload: &[u8],
        signature_header: &str,
        secret: &str,
        now: i64,
    ) -> Result<(), &'static str> {
        let mut timestamp: Option<i64> = None;
        let mut candidates = Vec::new();
        for part in signature_header.split(',') {
            let mut kv = part.split('=');
            let (Some(key), Some(value), None) = (kv.next(), kv.next(), kv.next()) else {
                return Err("invalid Stripe-Signature header");
            };
            if !valid_key(key)
                || value.is_empty()
                || value
                    .bytes()
                    .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
            {
                return Err("invalid Stripe-Signature header");
            }
            match key {
                "t" if timestamp.is_some() => {
                    return Err("invalid Stripe-Signature header");
                }
                "t" => timestamp = value.parse().ok(),
                "v1" => candidates.push(value),
                _ => {}
            }
        }
        let timestamp = timestamp.ok_or("missing timestamp in Stripe-Signature header")?;
        if now.abs_diff(timestamp) > SIGNATURE_TOLERANCE_SECS as u64 {
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
            Ok(())
        } else {
            Err("no v1 signature matched the computed HMAC")
        }
    }

    fn valid_key(key: &str) -> bool {
        !key.is_empty()
            && key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    }
}

/// How far a webhook's `t=` timestamp may drift from now before it's rejected as a possible replay.
/// Stripe's own guidance uses 5 minutes.
pub const SIGNATURE_TOLERANCE_SECS: i64 = 300;

/// Configuration for the Stripe integration.
/// Both fields are absent by default: a deployment with no `YORISHIRO_STRIPE_WEBHOOK_SECRET` set gets a 501 from the webhook endpoint instead of silently accepting unverifiable requests.
#[derive(Clone, Default)]
pub struct StripeConfig {
    pub webhook_secret: Option<String>,
    pub(crate) price_mapping: StripePriceMapping,
}

impl StripeConfig {
    pub fn from_env() -> Self {
        Self {
            webhook_secret: non_empty_env("YORISHIRO_STRIPE_WEBHOOK_SECRET"),
            price_mapping: StripePriceMapping::from_env(),
        }
    }
}

/// Returns 501 without a configured secret, 400 on a missing/invalid signature or malformed body, and 200 once the event has been applied (or was simply not one we act on).
///
/// Returns `impl IntoResponse` with raw status codes rather than going through `ApiError`: Stripe expects plain-text error bodies from webhooks, not the JSON `{"error": {...}}` envelope the rest of this API uses.
#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/stripe/webhook", params(("Stripe-Signature" = String, Header, description = "Stripe webhook signature")), request_body(content = String, content_type = "application/json"), responses((status = 200, description = "Event accepted"), (status = 400, description = "Plain-text validation error", content_type = "text/plain", body = String), (status = 500, description = "Event processing failed"), (status = 501, description = "Stripe billing is not configured", content_type = "text/plain", body = String)), security(()), tag = "enterprise"))]
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

    let event = match inbound::verify(&headers, &body, secret) {
        Ok(event) => event,
        Err(inbound::Error::MissingSignatureHeader) => {
            return (StatusCode::BAD_REQUEST, "missing Stripe-Signature header").into_response();
        }
        Err(inbound::Error::Signature(reason)) => {
            tracing::warn!(
                reason,
                "rejected Stripe webhook: signature verification failed"
            );
            return (StatusCode::BAD_REQUEST, reason).into_response();
        }
        Err(inbound::Error::InvalidJson(err)) => {
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

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::route_inventory::RouteDoc> {
    vec![crate::controllers::route_inventory::path_doc(
        __path_stripe_webhook,
    )]
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
async fn apply_stripe_event(
    ctx: &AppContext,
    config: &StripeConfig,
    event: inbound::VerifiedStripeEvent,
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
            tenant_billing::set_plan(&txn, tenant_id, plan.as_str()).await?;
            tenant_tenants::set_max_workspaces(&txn, tenant_id, caps.max_workspaces).await?;
        }
        "customer.subscription.deleted" => {
            let object = &event.data.object;
            let Some(tenant_id) = resolve_tenant_by_customer(&txn, object).await? else {
                return Ok(());
            };
            let caps = Plan::Free.caps();
            tenant_billing::set_plan(&txn, tenant_id, Plan::Free.as_str()).await?;
            tenant_tenants::set_max_workspaces(&txn, tenant_id, caps.max_workspaces).await?;
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
        .prefix("api/stripe")
        .add("/webhook", post(stripe_webhook))
}
