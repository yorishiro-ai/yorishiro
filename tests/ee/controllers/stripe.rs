mod inbound;

use std::sync::Arc;

use crate::requests::boot_request;
use crate::requests::boot_request_with;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use axum::http::{HeaderValue, StatusCode};
use chrono::Utc;
use loco_rs::app::AppContext;
use serde_json::json;
use serial_test::serial;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::edition::ee::controllers::middleware::edition::{LicenceClaims, LicenceState};
use yorishiro::edition::ee::models::tenant_billing;

const WEBHOOK_SECRET: &str = "whsec_test";

fn enable_stripe(ctx: &AppContext) {
    ctx.shared_store
        .insert(Arc::new(LicenceState::licensed(LicenceClaims {
            sub: "stripe-test".into(),
            plan: "pro".into(),
            exp: Utc::now().timestamp() + 3600,
        })));
}

/// Stripe as the apps boot with it.
fn stripe_overrides() -> [crate::requests::Override; 3] {
    [
        ("initializers.stripe.webhook_secret", WEBHOOK_SECRET.into()),
        ("initializers.stripe.price_pro", "price_pro".into()),
        ("initializers.stripe.price_team", "price_team".into()),
    ]
}

fn sign(secret: &str, timestamp: i64, payload: &[u8]) -> String {
    stripe_webhook::Webhook::generate_test_header(
        std::str::from_utf8(payload).unwrap(),
        secret,
        Some(timestamp),
    )
    .split_once(",v1=")
    .unwrap()
    .1
    .to_owned()
}

fn event_body(
    event_id: &str,
    event_type: &str,
    created: i64,
    object: &serde_json::Value,
) -> Vec<u8> {
    json!({
        "id": event_id,
        "type": event_type,
        "created": created,
        "livemode": false,
        "pending_webhooks": 1,
        "data": { "object": object }
    })
    .to_string()
    .into_bytes()
}

fn subscription_object(created: i64, customer_id: &str) -> serde_json::Value {
    json!({
        "id": "sub_fixture", "customer": customer_id, "created": created,
        "automatic_tax": {"enabled": false}, "billing_cycle_anchor": created,
        "billing_mode": {"type": "classic"}, "billing_schedules": [],
        "cancel_at_period_end": false, "collection_method": "charge_automatically",
        "currency": "usd", "discounts": [], "invoice_settings": {"issuer": {"type": "self"}},
        "livemode": false, "metadata": {}, "start_date": created, "status": "active",
        "items": {"object": "list", "has_more": false, "url": "/v1/subscription_items", "data": [{
            "id": "si_fixture", "subscription": "sub_fixture", "created": created,
            "current_period_start": created, "current_period_end": created + 86_400,
            "discounts": [], "metadata": {},
            "plan": {"id": "plan_fixture", "active": true, "billing_scheme": "per_unit",
                "created": created, "currency": "usd", "interval": "month",
                "interval_count": 1, "livemode": false, "usage_type": "licensed"},
            "price": {"id": "price_pro", "active": true, "billing_scheme": "per_unit",
                "created": created, "currency": "usd", "livemode": false,
                "metadata": {}, "product": "prod_fixture", "type": "recurring"}
        }]}
    })
}

fn subscription_updated_body(event_id: &str, created: i64, customer_id: &str) -> Vec<u8> {
    event_body(
        event_id,
        "customer.subscription.updated",
        created,
        &subscription_object(created, customer_id),
    )
}

fn subscription_deleted_body(event_id: &str, created: i64, customer_id: &str) -> Vec<u8> {
    let mut object = subscription_object(created, customer_id);
    object["status"] = json!("canceled");
    event_body(event_id, "customer.subscription.deleted", created, &object)
}

fn checkout_completed_body(event_id: &str, created: i64, tenant_id: Uuid) -> Vec<u8> {
    event_body(
        event_id,
        "checkout.session.completed",
        created,
        &json!({
            "id": "cs_test_fixture", "client_reference_id": tenant_id, "customer": "cus_checkout",
            "automatic_tax": {"enabled": false}, "created": created, "custom_fields": [],
            "custom_text": {}, "expires_at": created + 3600, "livemode": false,
            "mode": "subscription", "payment_method_types": ["card"],
            "payment_status": "paid", "shipping_options": []
        }),
    )
}

/// A duplicate delivery of an already-applied event must not re-apply it.
#[tokio::test]
#[serial(process_environment)]
async fn a_duplicate_event_id_is_not_reapplied() {
    // Captured before boot: a guard cannot be taken inside a booted test's scope.
    let changed = crate::EnvGuard::capture(&[
        "YORISHIRO_STRIPE_WEBHOOK_SECRET",
        "YORISHIRO_STRIPE_PRICE_PRO",
        "YORISHIRO_STRIPE_PRICE_TEAM",
    ]);
    boot_request_with::<App, _, _>(&stripe_overrides(), |request, ctx| async move {
        enable_stripe(&ctx);
        changed.set(
            "YORISHIRO_STRIPE_WEBHOOK_SECRET",
            "changed-after-config-load",
        );
        changed.set(
            "YORISHIRO_STRIPE_PRICE_PRO",
            "changed-price-after-config-load",
        );
        changed.set(
            "YORISHIRO_STRIPE_PRICE_TEAM",
            "changed-team-after-config-load",
        );
        let tenant_id = TestTenant::create(&request, &ctx, TenantArgs::default())
            .await
            .tenant_id;
        tenant_billing::link_stripe_customer(&ctx.db, tenant_id, "cus_1")
            .await
            .unwrap();

        let body = subscription_updated_body("evt_1", Utc::now().timestamp(), "cus_1");
        let now = Utc::now().timestamp();
        let signature = sign(WEBHOOK_SECRET, now, &body);

        let first = request
            .post("/api/stripe/webhook")
            .add_header("stripe-signature", format!("t={now},v1={signature}"))
            .bytes(body.clone().into())
            .await;
        assert_eq!(
            first.status_code(),
            StatusCode::OK,
            "response: {:?}",
            first.text()
        );
        let after_first = tenant_billing::get_billing(&ctx.db, tenant_id)
            .await
            .unwrap()
            .and_then(|r| r.plan);
        assert_eq!(after_first.as_deref(), Some("pro"));

        // Downgrade directly, bypassing the webhook, so a re-application of the duplicate delivery below would be observable as a plan flip back to `pro`.
        tenant_billing::set_plan(&ctx.db, tenant_id, "free")
            .await
            .unwrap();

        let retry = request
            .post("/api/stripe/webhook")
            .add_header("stripe-signature", format!("t={now},v1={signature}"))
            .bytes(body.into())
            .await;
        assert_eq!(
            retry.status_code(),
            200,
            "a retried delivery of the same event id must still 200 (so Stripe stops \
                 retrying): {:?}",
            retry.text()
        );
        let after_retry = tenant_billing::get_billing(&ctx.db, tenant_id)
            .await
            .unwrap()
            .and_then(|r| r.plan);
        assert_eq!(
            after_retry.as_deref(),
            Some("free"),
            "the duplicate must not have re-applied the plan change"
        );
    })
    .await;
}

/// Cancelling a subscription changes billing without overwriting operational caps.
#[tokio::test]
async fn a_cancellation_returns_the_tenant_to_free() {
    let overrides = [("initializers.plan_policy_enabled", "true".into())];
    boot_request_with::<App, _, _>(
        &[&stripe_overrides()[..], &overrides[..]].concat(),
        |request, ctx| async move {
            enable_stripe(&ctx);
            let tenant_id = TestTenant::create(&request, &ctx, TenantArgs::default())
                .await
                .tenant_id;
            tenant_billing::link_stripe_customer(&ctx.db, tenant_id, "cus_cancel")
                .await
                .unwrap();
            crate::requests::fixtures::set_workspace_cap(&ctx, tenant_id, 17).await;

            // Land on Pro first, so the downgrade has somewhere to fall from.
            let up_body = subscription_updated_body("evt_up", 1_000, "cus_cancel");
            let now = Utc::now().timestamp();
            let up_sig = sign(WEBHOOK_SECRET, now, &up_body);
            let up = request
                .post("/api/stripe/webhook")
                .add_header("stripe-signature", format!("t={now},v1={up_sig}"))
                .bytes(up_body.into())
                .await;
            assert_eq!(
                up.status_code(),
                StatusCode::OK,
                "response: {:?}",
                up.text()
            );

            let del_body = subscription_deleted_body("evt_del", 2_000, "cus_cancel");
            let now = Utc::now().timestamp();
            let del_sig = sign(WEBHOOK_SECRET, now, &del_body);
            let del = request
                .post("/api/stripe/webhook")
                .add_header("stripe-signature", format!("t={now},v1={del_sig}"))
                .bytes(del_body.into())
                .await;
            assert_eq!(
                del.status_code(),
                StatusCode::OK,
                "response: {:?}",
                del.text()
            );

            let billing_record = tenant_billing::get_billing(&ctx.db, tenant_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(billing_record.plan.as_deref(), Some("free"));

            let tenant = yorishiro::models::tenant_tenants::Model::find_by_id(&ctx.db, tenant_id)
                .await
                .unwrap();
            assert_eq!(
                tenant.max_workspaces,
                Some(17),
                "billing changes must not persist plan quotas over operational caps"
            );
        },
    )
    .await;
}

#[tokio::test]
async fn disabled_policy_grandfathers_workspace_cap_on_stripe_changes() {
    let overrides = [("initializers.plan_policy_enabled", "false".into())];
    boot_request_with::<App, _, _>(
        &[&stripe_overrides()[..], &overrides[..]].concat(),
        |request, ctx| async move {
            enable_stripe(&ctx);
            let tenant_id = TestTenant::create(&request, &ctx, TenantArgs::default())
                .await
                .tenant_id;
            // Fault fixture: a legacy custom cap has no public mutation API and must survive webhook changes.
            crate::requests::fixtures::set_workspace_cap(&ctx, tenant_id, 17).await;
            tenant_billing::link_stripe_customer(&ctx.db, tenant_id, "cus_grandfathered")
                .await
                .unwrap();
            for (event_id, body) in [
                (
                    "evt_gf_up",
                    subscription_updated_body("evt_gf_up", 1, "cus_grandfathered"),
                ),
                (
                    "evt_gf_del",
                    subscription_deleted_body("evt_gf_del", 2, "cus_grandfathered"),
                ),
            ] {
                let now = Utc::now().timestamp();
                let sig = sign(WEBHOOK_SECRET, now, &body);
                let response = request
                    .post("/api/stripe/webhook")
                    .add_header("stripe-signature", format!("t={now},v1={sig}"))
                    .bytes(body.into())
                    .await;
                assert_eq!(
                    response.status_code(),
                    StatusCode::OK,
                    "{event_id}: {}",
                    response.text()
                );
            }
            let billing = tenant_billing::get_billing(&ctx.db, tenant_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(billing.plan.as_deref(), Some("free"));
            let tenant = yorishiro::models::tenant_tenants::Model::find_by_id(&ctx.db, tenant_id)
                .await
                .unwrap();
            assert_eq!(tenant.max_workspaces, Some(17));
        },
    )
    .await;
}

/// With no webhook secret configured there is no way to tell a genuine Stripe delivery from anything else, so the endpoint refuses rather than accepting unverifiable requests.
#[tokio::test]
async fn an_unconfigured_webhook_refuses_rather_than_accepting() {
    boot_request::<App, _, _>(|request, ctx| async move {
        enable_stripe(&ctx);
        let body = subscription_updated_body("evt_x", 1_000, "cus_x");
        let response = request
            .post("/api/stripe/webhook")
            .add_header("stripe-signature", "t=1000,v1=deadbeef")
            .bytes(body.into())
            .await;
        assert_eq!(
            response.status_code(),
            501,
            "an unconfigured webhook must refuse, never accept an unverifiable request: {:?}",
            response.text()
        );
    })
    .await;
}

/// Signature failures return a plain-text 400.
#[tokio::test]
async fn malformed_signature_headers_have_exact_plain_text_rejections() {
    boot_request_with::<App, _, _>(&stripe_overrides(), |request, ctx| async move {
        enable_stripe(&ctx);
        let body = subscription_updated_body("evt_header", 1_000, "cus_header");
        let now = Utc::now().timestamp();
        let signature = sign(WEBHOOK_SECRET, now, &body);

        let missing = request
            .post("/api/stripe/webhook")
            .bytes(body.clone().into())
            .await;
        assert_eq!(missing.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(missing.text(), "missing Stripe-Signature header");

        let invalid = request
            .post("/api/stripe/webhook")
            .add_header("stripe-signature", format!("t={now},v1=wrong"))
            .bytes(body.clone().into())
            .await;
        assert_eq!(invalid.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(invalid.text(), "invalid Stripe signature");

        let malformed = request
            .post("/api/stripe/webhook")
            .add_header(
                "stripe-signature",
                format!("t={now},v1={signature},malformed"),
            )
            .bytes(body.clone().into())
            .await;
        assert_eq!(malformed.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(malformed.text(), "invalid Stripe signature");

        let mut invalid_utf8 = HeaderValue::from_bytes(&[0xff]).unwrap();
        invalid_utf8.set_sensitive(true);
        let invalid_utf8_response = request
            .post("/api/stripe/webhook")
            .add_header("stripe-signature", invalid_utf8)
            .bytes(body.into())
            .await;
        assert_eq!(invalid_utf8_response.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(
            invalid_utf8_response.text(),
            "missing Stripe-Signature header"
        );
    })
    .await;
}

/// A valid signature is checked before malformed JSON is reported.
#[tokio::test]
async fn malformed_json_is_rejected_after_signature_verification() {
    boot_request_with::<App, _, _>(&stripe_overrides(), |request, ctx| async move {
        enable_stripe(&ctx);
        let body = b"not json".to_vec();
        let now = Utc::now().timestamp();
        let valid_signature = sign(WEBHOOK_SECRET, now, &body);

        let valid = request
            .post("/api/stripe/webhook")
            .add_header("stripe-signature", format!("t={now},v1={valid_signature}"))
            .bytes(body.clone().into())
            .await;
        assert_eq!(valid.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(valid.text(), "invalid Stripe event");

        let invalid_signature = request
            .post("/api/stripe/webhook")
            .add_header("stripe-signature", format!("t={now},v1=wrong"))
            .bytes(body.into())
            .await;
        assert_eq!(invalid_signature.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(invalid_signature.text(), "invalid Stripe signature");
    })
    .await;
}

/// A delayed subscription event is accepted but cannot undo a newer applied event.
#[tokio::test]
async fn a_stale_subscription_event_is_not_reapplied() {
    boot_request_with::<App, _, _>(&stripe_overrides(), |request, ctx| async move {
        enable_stripe(&ctx);
        let tenant_id = TestTenant::create(&request, &ctx, TenantArgs::default())
            .await
            .tenant_id;
        tenant_billing::link_stripe_customer(&ctx.db, tenant_id, "cus_stale")
            .await
            .unwrap();

        let newer_body = subscription_updated_body("evt_newer", 2_000, "cus_stale");
        let now = Utc::now().timestamp();
        let newer = request
            .post("/api/stripe/webhook")
            .add_header(
                "stripe-signature",
                format!("t={now},v1={}", sign(WEBHOOK_SECRET, now, &newer_body)),
            )
            .bytes(newer_body.into())
            .await;
        assert_eq!(newer.status_code(), StatusCode::OK);

        let stale_body = subscription_deleted_body("evt_stale", 1_000, "cus_stale");
        let stale = request
            .post("/api/stripe/webhook")
            .add_header(
                "stripe-signature",
                format!("t={now},v1={}", sign(WEBHOOK_SECRET, now, &stale_body)),
            )
            .bytes(stale_body.into())
            .await;
        assert_eq!(stale.status_code(), StatusCode::OK);
        assert_eq!(stale.text(), "");
        assert_eq!(
            tenant_billing::get_billing(&ctx.db, tenant_id)
                .await
                .unwrap()
                .and_then(|record| record.plan),
            Some("pro".into())
        );
    })
    .await;
}

/// Checkout completion links the Stripe customer so a later subscription event can resolve its tenant.
#[tokio::test]
async fn checkout_completion_links_the_stripe_customer() {
    boot_request_with::<App, _, _>(&stripe_overrides(), |request, ctx| async move {
        enable_stripe(&ctx);
        let tenant_id = TestTenant::create(&request, &ctx, TenantArgs::default())
            .await
            .tenant_id;
        let body = checkout_completed_body("evt_checkout", 1_000, tenant_id);
        let now = Utc::now().timestamp();
        let subscription = subscription_updated_body("evt_before_checkout", 1_001, "cus_checkout");
        let signature = format!("t={now},v1={}", sign(WEBHOOK_SECRET, now, &subscription));
        let early = request
            .post("/api/stripe/webhook")
            .add_header("stripe-signature", signature.clone())
            .bytes(subscription.clone().into())
            .await;
        assert_eq!(early.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
        let response = request
            .post("/api/stripe/webhook")
            .add_header(
                "stripe-signature",
                format!("t={now},v1={}", sign(WEBHOOK_SECRET, now, &body)),
            )
            .bytes(body.into())
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
        assert_eq!(response.text(), "");
        assert_eq!(
            tenant_billing::get_billing(&ctx.db, tenant_id)
                .await
                .unwrap()
                .and_then(|record| record.stripe_customer_id),
            Some("cus_checkout".into())
        );
        let retried = request
            .post("/api/stripe/webhook")
            .add_header("stripe-signature", signature)
            .bytes(subscription.into())
            .await;
        assert_eq!(retried.status_code(), StatusCode::OK);
        assert_eq!(
            tenant_billing::get_billing(&ctx.db, tenant_id)
                .await
                .unwrap()
                .and_then(|record| record.plan),
            Some("pro".into())
        );
    })
    .await;
}
