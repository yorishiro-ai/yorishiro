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
    use axum::http::HeaderMap;
    use stripe_webhook::{Webhook, WebhookError};

    #[derive(Debug)]
    pub enum Error {
        MissingSignatureHeader,
        InvalidSignature,
        InvalidPayload,
    }

    pub use crate::edition::ee::dtos::stripe::VerifiedStripeEvent;

    pub(super) fn verify(
        headers: &HeaderMap,
        payload: &[u8],
        secret: &str,
    ) -> Result<VerifiedStripeEvent, Error> {
        verify_at(
            headers,
            payload,
            secret,
            crate::data::time::now().timestamp(),
        )
    }

    /// # Errors
    /// Returns an error for a missing header, invalid signature, or invalid event.
    pub fn verify_at(
        headers: &HeaderMap,
        payload: &[u8],
        secret: &str,
        now: i64,
    ) -> Result<VerifiedStripeEvent, Error> {
        let signature = headers
            .get("stripe-signature")
            .and_then(|header| header.to_str().ok())
            .ok_or(Error::MissingSignatureHeader)?;
        // The SDK checks tolerance with signed subtraction and abs(), which can overflow.
        if signature
            .split(',')
            .filter_map(|part| part.strip_prefix("t="))
            .filter_map(|timestamp| timestamp.parse::<i64>().ok())
            .any(|timestamp| now.abs_diff(timestamp) > i64::MAX.cast_unsigned())
        {
            return Err(Error::InvalidSignature);
        }
        let payload = std::str::from_utf8(payload).map_err(|_| Error::InvalidPayload)?;
        Webhook::construct_event_with_timestamp(payload, signature, secret, now).map_err(
            |error| match error {
                WebhookError::BadParse(_) => Error::InvalidPayload,
                _ => Error::InvalidSignature,
            },
        )?;
        // Billing uses the original object fields across Stripe API versions.
        serde_json::from_str(payload).map_err(|_| Error::InvalidPayload)
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
    let Ok(config) = StripeConfig::from_config(&ctx.config) else {
        tracing::error!("invalid initializers.stripe configuration");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Stripe configuration is invalid",
        )
            .into_response();
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
        Err(inbound::Error::InvalidSignature) => {
            return (StatusCode::BAD_REQUEST, "invalid Stripe signature").into_response();
        }
        Err(inbound::Error::InvalidPayload) => {
            return (StatusCode::BAD_REQUEST, "invalid Stripe event").into_response();
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
pub(crate) fn openapi_docs() -> Vec<crate::controllers::openapi::route_inventory::RouteDoc> {
    vec![crate::controllers::openapi::route_inventory::path_doc(
        __path_stripe_webhook,
    )]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/stripe")
        .add("/webhook", post(stripe_webhook))
}
