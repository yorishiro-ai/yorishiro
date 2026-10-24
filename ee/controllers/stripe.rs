//! The single public HTTP entry point for Stripe webhooks.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::edition::ee::data::stripe::StripeConfig;
use crate::edition::ee::models::stripe_events;

pub mod inbound {
    use super::SIGNATURE_TOLERANCE_SECS;
    use axum::http::HeaderMap;

    use crate::edition::ee::controllers::hmac_sign;

    pub enum Error {
        MissingSignatureHeader,
        Signature(&'static str),
        InvalidJson(serde_json::Error),
    }

    pub use crate::edition::ee::dtos::stripe::VerifiedStripeEvent;

    pub(super) fn verify(
        headers: &HeaderMap,
        payload: &[u8],
        secret: &str,
    ) -> Result<VerifiedStripeEvent, Error> {
        verify_at(headers, payload, secret, crate::time::now().timestamp())
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

/// Returns 501 without a configured secret, 400 on a missing/invalid signature or malformed body, and 200 once the event has been applied (or was simply not one we act on).
///
/// Returns `impl IntoResponse` with raw status codes rather than going through `ApiError`: Stripe expects plain-text error bodies from webhooks, not the JSON `{"error": {...}}` envelope the rest of this API uses.
#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/stripe/webhook", params(("Stripe-Signature" = String, Header, description = "Stripe webhook signature")), request_body(content = String, content_type = "application/json"), responses((status = 200, description = "Event accepted"), (status = 400, description = "Plain-text validation error", content_type = "text/plain", body = String), (status = 500, description = "Event processing failed"), (status = 501, description = "Stripe billing is not configured", content_type = "text/plain", body = String)), security(()), tag = "enterprise"))]
async fn stripe_webhook(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let config = match StripeConfig::from_config(&ctx.config) {
        Ok(config) => config,
        Err(_) => {
            tracing::error!("invalid initializers.stripe configuration");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Stripe configuration is invalid",
            )
                .into_response();
        }
    };
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

    match stripe_events::apply_stripe_event(&ctx, &config, event).await {
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

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/stripe")
        .add("/webhook", post(stripe_webhook))
}
