use crate::http::controllers::stripe::{StripeConfig, stripe_webhook, verify_stripe_signature};
use crate::services::billing;
use crate::services::plan::StripePriceMapping;
use crate::state::HostedState;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{TimeZone, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::json;
use sha2::Sha256;
use sqlx::PgPool;
use tower::ServiceExt;
use yorishiro_core::repositories::tenancy;

use crate::tests::test_helpers;

type HmacSha256 = Hmac<Sha256>;

fn sign(secret: &str, timestamp: i64, payload: &[u8]) -> String {
    let mut signed_payload = format!("{timestamp}.").into_bytes();
    signed_payload.extend_from_slice(payload);
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(&signed_payload);
    let sig = mac.finalize().into_bytes();
    sig.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn accepts_a_correctly_signed_recent_payload() {
    let secret = "whsec_test";
    let payload = br#"{"type":"ping"}"#;
    let now = Utc::now().timestamp();
    let signature = sign(secret, now, payload);
    let header = format!("t={now},v1={signature}");

    verify_stripe_signature(payload, &header, secret).unwrap();
}

#[test]
fn rejects_a_signature_computed_with_the_wrong_secret() {
    let payload = br#"{"type":"ping"}"#;
    let now = Utc::now().timestamp();
    let signature = sign("whsec_wrong", now, payload);
    let header = format!("t={now},v1={signature}");

    let err = verify_stripe_signature(payload, &header, "whsec_test").unwrap_err();
    assert_eq!(err, "no v1 signature matched the computed HMAC");
}

#[test]
fn rejects_a_stale_timestamp() {
    let secret = "whsec_test";
    let payload = br#"{"type":"ping"}"#;
    let stale = Utc::now().timestamp() - 3600;
    let signature = sign(secret, stale, payload);
    let header = format!("t={stale},v1={signature}");

    let err = verify_stripe_signature(payload, &header, secret).unwrap_err();
    assert_eq!(
        err,
        "Stripe-Signature timestamp is outside the allowed tolerance"
    );
}

#[test]
fn rejects_a_header_with_no_v1_entries() {
    let now = Utc::now().timestamp();
    let header = format!("t={now}");
    let err = verify_stripe_signature(b"{}", &header, "whsec_test").unwrap_err();
    assert_eq!(err, "missing v1 signature in Stripe-Signature header");
}

#[test]
fn rejects_a_tampered_payload() {
    let secret = "whsec_test";
    let payload = br#"{"type":"ping"}"#;
    let now = Utc::now().timestamp();
    let signature = sign(secret, now, payload);
    let header = format!("t={now},v1={signature}");

    let tampered = br#"{"type":"pong"}"#;
    let err = verify_stripe_signature(tampered, &header, secret).unwrap_err();
    assert_eq!(err, "no v1 signature matched the computed HMAC");
}

const WEBHOOK_SECRET: &str = "whsec_test";

fn router(pool: PgPool) -> Router {
    let state = HostedState {
        tenant_db: yorishiro_core::db::TenantDb::new(pool.clone()),
        stripe_config: StripeConfig {
            webhook_secret: Some(WEBHOOK_SECRET.into()),
            price_mapping: StripePriceMapping {
                pro_price_id: Some("price_pro".into()),
                team_price_id: Some("price_team".into()),
            },
        },
        ..test_helpers::hosted_state(pool)
    };
    Router::new()
        .route(
            "/hosted/stripe/webhook",
            axum::routing::post(stripe_webhook),
        )
        .with_state(state)
}

/// Builds a signed webhook request body for a `customer.subscription.deleted` event.
fn subscription_deleted_body(event_id: &str, created: i64, customer_id: &str) -> Vec<u8> {
    json!({
        "id": event_id,
        "type": "customer.subscription.deleted",
        "created": created,
        "data": { "object": { "customer": customer_id } }
    })
    .to_string()
    .into_bytes()
}

/// Reads the tenant's workspace cap, which the plan change writes alongside the plan itself.
async fn tenant_max_workspaces(pool: &PgPool, tenant_id: uuid::Uuid) -> Option<i32> {
    let mut conn = pool.acquire().await.unwrap();
    yorishiro_core::repositories::tenancy::get_tenant(&mut conn, tenant_id)
        .await
        .unwrap()
        .max_workspaces
}

/// Builds a signed webhook request body for a `customer.subscription.updated` event.
fn subscription_updated_body(event_id: &str, created: i64, customer_id: &str) -> Vec<u8> {
    json!({
        "id": event_id,
        "type": "customer.subscription.updated",
        "created": created,
        "data": {
            "object": {
                "customer": customer_id,
                "items": { "data": [{ "price": { "id": "price_pro" } }] }
            }
        }
    })
    .to_string()
    .into_bytes()
}

/// Builds a signed webhook request body for a `checkout.session.completed` event.
fn checkout_session_completed_body(
    event_id: &str,
    created: i64,
    tenant_id: uuid::Uuid,
    customer_id: &str,
) -> Vec<u8> {
    json!({
        "id": event_id,
        "type": "checkout.session.completed",
        "created": created,
        "data": {
            "object": {
                "client_reference_id": tenant_id.to_string(),
                "customer": customer_id,
            }
        }
    })
    .to_string()
    .into_bytes()
}

async fn post_webhook(app: &Router, body: Vec<u8>) -> StatusCode {
    let now = Utc::now().timestamp();
    let signature = sign(WEBHOOK_SECRET, now, &body);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/hosted/stripe/webhook")
                .header("stripe-signature", format!("t={now},v1={signature}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    response.status()
}

/// Reads the tenant's plan from this repo's own billing table. `identity.tenants` no longer
/// carries a plan column -- the community edition has no notion of one -- so the assertions
/// below go through `billing` rather than through a tenant record.
async fn tenant_plan(pool: &PgPool, tenant_id: uuid::Uuid) -> Option<String> {
    billing::get_billing(pool, tenant_id)
        .await
        .unwrap()
        .and_then(|record| record.plan)
}

#[sqlx::test(migrator = "test_helpers::COMBINED_MIGRATOR")]
async fn a_duplicate_event_id_is_not_reapplied(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    billing::link_stripe_customer(&pool, tenant.id, "cus_1")
        .await
        .unwrap();

    let app = router(pool.clone());
    let created = Utc::now().timestamp();
    let body = subscription_updated_body("evt_1", created, "cus_1");

    assert_eq!(
        post_webhook(&app, body.clone()).await,
        StatusCode::OK,
        "first delivery should apply"
    );
    let after_first = tenant_plan(&pool, tenant.id).await;
    assert_eq!(after_first.as_deref(), Some("pro"));

    // Downgrade the tenant to `free` directly, bypassing the webhook, so a re-application of the
    // duplicate delivery below would be observable as a plan flip back to `pro`.
    billing::set_plan(&pool, tenant.id, "free").await.unwrap();

    assert_eq!(
        post_webhook(&app, body).await,
        StatusCode::OK,
        "a retried delivery of the same event id must still 200 (so Stripe stops retrying)"
    );
    let after_retry = tenant_plan(&pool, tenant.id).await;
    assert_eq!(
        after_retry.as_deref(),
        Some("free"),
        "the duplicate must not have re-applied the plan change"
    );
}

#[sqlx::test(migrator = "test_helpers::COMBINED_MIGRATOR")]
async fn an_out_of_order_delivery_does_not_undo_a_newer_event(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    billing::link_stripe_customer(&pool, tenant.id, "cus_2")
        .await
        .unwrap();

    let app = router(pool.clone());
    let base = Utc
        .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .unwrap()
        .timestamp();

    // The newer event (created later) arrives first -- plausible under Stripe's own
    // no-ordering-guarantee, or simply because the older one was delayed/retried.
    let newer_body = subscription_updated_body("evt_newer", base + 100, "cus_2");
    assert_eq!(post_webhook(&app, newer_body).await, StatusCode::OK);
    let after_newer = tenant_plan(&pool, tenant.id).await;
    assert_eq!(after_newer.as_deref(), Some("pro"));

    billing::set_plan(&pool, tenant.id, "free").await.unwrap();

    // The older, stale event now arrives (a different event id, so it isn't caught by the
    // duplicate-event-id guard alone) -- it must not be allowed to move the plan again.
    let older_body = subscription_updated_body("evt_older", base, "cus_2");
    assert_eq!(post_webhook(&app, older_body).await, StatusCode::OK);
    let after_stale = tenant_plan(&pool, tenant.id).await;
    assert_eq!(
        after_stale.as_deref(),
        Some("free"),
        "a delivery older than the last-applied event for this customer must not be re-applied"
    );
}

#[sqlx::test(migrator = "test_helpers::COMBINED_MIGRATOR")]
async fn events_for_different_customers_do_not_interfere(pool: PgPool) {
    let tenant_a = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let tenant_b = tenancy::create_tenant(&pool, "beta", None).await.unwrap();
    billing::link_stripe_customer(&pool, tenant_a.id, "cus_a")
        .await
        .unwrap();
    billing::link_stripe_customer(&pool, tenant_b.id, "cus_b")
        .await
        .unwrap();

    let app = router(pool.clone());
    let base = Utc::now().timestamp();

    assert_eq!(
        post_webhook(&app, subscription_updated_body("evt_a", base, "cus_a")).await,
        StatusCode::OK
    );
    // An older `created` for a *different* customer must apply normally -- staleness is scoped
    // per customer, not global.
    assert_eq!(
        post_webhook(
            &app,
            subscription_updated_body("evt_b", base - 1000, "cus_b")
        )
        .await,
        StatusCode::OK
    );

    let tenant_a = tenant_plan(&pool, tenant_a.id).await;
    let tenant_b = tenant_plan(&pool, tenant_b.id).await;
    assert_eq!(tenant_a.as_deref(), Some("pro"));
    assert_eq!(tenant_b.as_deref(), Some("pro"));
}

/// A `checkout.session.completed` event must not set a per-customer staleness floor: Stripe does
/// not guarantee delivery order between it and the `customer.subscription.created` event for the
/// same purchase, and the subscription event can carry an earlier `created` timestamp. If the
/// checkout event's `customer_id` were recorded for ordering, that earlier subscription event
/// would be wrongly rejected as stale and the tenant would never receive its purchased plan.
#[sqlx::test(migrator = "test_helpers::COMBINED_MIGRATOR")]
async fn a_checkout_completion_does_not_block_an_earlier_created_subscription_event(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let app = router(pool.clone());
    let base = Utc::now().timestamp();

    // The subscription event's own `created` predates the checkout event's, as can genuinely
    // happen, and the checkout event is delivered first.
    let checkout_body =
        checkout_session_completed_body("evt_checkout", base, tenant.id, "cus_checkout");
    assert_eq!(post_webhook(&app, checkout_body).await, StatusCode::OK);

    let subscription_body =
        subscription_updated_body("evt_sub_created", base - 100, "cus_checkout");
    assert_eq!(post_webhook(&app, subscription_body).await, StatusCode::OK);

    let tenant = tenant_plan(&pool, tenant.id).await;
    assert_eq!(
        tenant.as_deref(),
        Some("pro"),
        "the subscription event must still apply even though checkout.session.completed \
         landed first with a later `created` timestamp"
    );
}

/// Cancelling a subscription has to put the tenant back on Free -- both the plan and the
/// workspace cap that comes with it. Missing the cap would leave a cancelled tenant with a paid
/// tier's limits, which is the expensive direction to get wrong.
#[sqlx::test(migrator = "test_helpers::COMBINED_MIGRATOR")]
async fn a_cancellation_returns_the_tenant_to_free(pool: PgPool) {
    let tenant = yorishiro_core::repositories::tenancy::create_tenant(&pool, "acme", None)
        .await
        .unwrap();
    billing::link_stripe_customer(&pool, tenant.id, "cus_cancel")
        .await
        .unwrap();

    let app = router(pool.clone());

    // Land on Pro first, so the downgrade has somewhere to fall from.
    assert_eq!(
        post_webhook(
            &app,
            subscription_updated_body("evt_up", 1_000, "cus_cancel")
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(tenant_plan(&pool, tenant.id).await.as_deref(), Some("pro"));
    assert_eq!(tenant_max_workspaces(&pool, tenant.id).await, Some(5));

    assert_eq!(
        post_webhook(
            &app,
            subscription_deleted_body("evt_del", 2_000, "cus_cancel")
        )
        .await,
        StatusCode::OK
    );

    assert_eq!(tenant_plan(&pool, tenant.id).await.as_deref(), Some("free"));
    assert_eq!(
        tenant_max_workspaces(&pool, tenant.id).await,
        Some(1),
        "a cancelled tenant must drop to Free's workspace cap, not keep the paid one"
    );
}

/// A cancellation for a customer nobody is linked to must be accepted and ignored, not error.
/// Stripe retries anything it does not get a 2xx for, so returning an error here would have it
/// redeliver the same event indefinitely.
#[sqlx::test(migrator = "test_helpers::COMBINED_MIGRATOR")]
async fn a_cancellation_for_an_unknown_customer_is_accepted_and_ignored(pool: PgPool) {
    let app = router(pool.clone());

    assert_eq!(
        post_webhook(
            &app,
            subscription_deleted_body("evt_orphan", 1_000, "cus_nobody")
        )
        .await,
        StatusCode::OK
    );
}

/// Billing is opt-in. With no webhook secret configured there is no way to tell a genuine Stripe
/// delivery from anything else, so the endpoint refuses rather than accepting unverifiable
/// requests -- if this ever started returning 200, a forged body would be applied to real
/// tenants.
#[sqlx::test(migrator = "test_helpers::COMBINED_MIGRATOR")]
async fn an_unconfigured_webhook_refuses_rather_than_accepting(pool: PgPool) {
    let state = HostedState {
        tenant_db: yorishiro_core::db::TenantDb::new(pool.clone()),
        stripe_config: StripeConfig {
            webhook_secret: None,
            price_mapping: StripePriceMapping {
                pro_price_id: Some("price_pro".into()),
                team_price_id: Some("price_team".into()),
            },
        },
        ..test_helpers::hosted_state(pool)
    };
    let app = Router::new()
        .route(
            "/hosted/stripe/webhook",
            axum::routing::post(stripe_webhook),
        )
        .with_state(state);

    assert_eq!(
        post_webhook(&app, subscription_updated_body("evt_x", 1_000, "cus_x")).await,
        StatusCode::NOT_IMPLEMENTED,
        "an unconfigured webhook must refuse, never accept an unverifiable request"
    );
}
