use super::boot_request_isolated as boot_request;
use super::fixtures::{TenantArgs, TestTenant};
use axum::http::{HeaderValue, StatusCode};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ActiveValue};
use serde_json::json;
use serial_test::serial;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::edition::ee::controllers::middleware::edition::LicenceState;
use yorishiro::edition::ee::models::tenant_billing;
use yorishiro::models::_entities::tenant_tenants;

const WEBHOOK_SECRET: &str = "whsec_test";

fn require_licence(ctx: &loco_rs::app::AppContext) {
    assert!(
        ctx.shared_store
            .get::<std::sync::Arc<LicenceState>>()
            .expect("production boot installs licence state")
            .is_active(),
        "licensed Stripe tests require a valid YORISHIRO_LICENSE_KEY at boot"
    );
}

/// The shipped Loco config interpolates these variables before booting each test app.
/// `#[serial(process_environment)]` on every test in this suite makes that safe.
async fn with_stripe_env<T>(fut: impl std::future::Future<Output = T>) -> T {
    let guard = crate::EnvGuard::capture(&[
        crate::env::TestEnv::StripeWebhookSecret.name(),
        crate::env::TestEnv::StripePricePro.name(),
        crate::env::TestEnv::StripePriceTeam.name(),
    ]);
    guard.set(
        crate::env::TestEnv::StripeWebhookSecret.name(),
        WEBHOOK_SECRET,
    );
    guard.set(crate::env::TestEnv::StripePricePro.name(), "price_pro");
    guard.set(crate::env::TestEnv::StripePriceTeam.name(), "price_team");
    fut.await
}
fn sign(secret: &str, timestamp: i64, payload: &[u8]) -> String {
    let mut signed_payload = format!("{timestamp}.").into_bytes();
    signed_payload.extend_from_slice(payload);
    yorishiro::edition::ee::controllers::hmac_sign::sign(secret.as_bytes(), &signed_payload)
}

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

fn checkout_completed_body(event_id: &str, created: i64, tenant_id: Uuid) -> Vec<u8> {
    json!({
        "id": event_id,
        "type": "checkout.session.completed",
        "created": created,
        "data": {
            "object": {
                "client_reference_id": tenant_id,
                "customer": "cus_checkout"
            }
        }
    })
    .to_string()
    .into_bytes()
}

/// A duplicate delivery of an already-applied event must not re-apply it.
#[tokio::test]
#[serial(process_environment)]
async fn a_duplicate_event_id_is_not_reapplied() {
    with_stripe_env(async {
        boot_request::<App, _, _>(|request, ctx| async move {
            require_licence(&ctx);
            let changed = crate::EnvGuard::capture(&[
                crate::env::TestEnv::StripeWebhookSecret.name(),
                crate::env::TestEnv::StripePricePro.name(),
                crate::env::TestEnv::StripePriceTeam.name(),
            ]);
            changed.set(
                crate::env::TestEnv::StripeWebhookSecret.name(),
                "changed-after-config-load",
            );
            changed.set(
                crate::env::TestEnv::StripePricePro.name(),
                "changed-price-after-config-load",
            );
            changed.set(
                crate::env::TestEnv::StripePriceTeam.name(),
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
    })
    .await;
}

/// Cancelling a subscription must put the tenant back on Free: both the plan and the workspace cap that comes with it.
#[tokio::test]
#[serial(process_environment)]
async fn a_cancellation_returns_the_tenant_to_free() {
    let policy = crate::EnvGuard::capture(&[crate::env::TestEnv::PlanPolicyEnabled.name()]);
    policy.set(crate::env::TestEnv::PlanPolicyEnabled.name(), "true");
    with_stripe_env(async {
        boot_request::<App, _, _>(|request, ctx| async move {
            require_licence(&ctx);
            let tenant_id = TestTenant::create(&request, &ctx, TenantArgs::default())
                .await
                .tenant_id;
            tenant_billing::link_stripe_customer(&ctx.db, tenant_id, "cus_cancel")
                .await
                .unwrap();

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
                Some(1),
                "a cancelled tenant must drop to Free's workspace cap, not keep the paid one"
            );
        })
        .await;
    })
    .await;
}

#[tokio::test]
#[serial(process_environment)]
async fn disabled_policy_grandfathers_workspace_cap_on_stripe_changes() {
    let policy = crate::EnvGuard::capture(&[crate::env::TestEnv::PlanPolicyEnabled.name()]);
    policy.remove(crate::env::TestEnv::PlanPolicyEnabled.name());
    with_stripe_env(async {
        boot_request::<App, _, _>(|request, ctx| async move {
            require_licence(&ctx);
            let tenant_id = TestTenant::create(&request, &ctx, TenantArgs::default())
                .await
                .tenant_id;
            // Fault fixture: a legacy custom cap has no public mutation API and must survive webhook changes.
            tenant_tenants::ActiveModel {
                id: ActiveValue::Set(tenant_id),
                max_workspaces: ActiveValue::Set(Some(17)),
                ..Default::default()
            }
            .update(&ctx.db)
            .await
            .unwrap();
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
        })
        .await;
    })
    .await;
}

/// With no webhook secret configured there is no way to tell a genuine Stripe delivery from anything else, so the endpoint refuses rather than accepting unverifiable requests.
#[tokio::test]
#[serial(process_environment)]
async fn an_unconfigured_webhook_refuses_rather_than_accepting() {
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
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

/// A tampered payload (signature computed over a different body) must be rejected.
#[tokio::test]
#[serial(process_environment)]
async fn a_tampered_payload_is_rejected() {
    with_stripe_env(async {
        boot_request::<App, _, _>(|request, ctx| async move {
            require_licence(&ctx);
            let body = subscription_updated_body("evt_tamper", Utc::now().timestamp(), "cus_t");
            let now = Utc::now().timestamp();
            let signature = sign(WEBHOOK_SECRET, now, &body);

            let tampered =
                subscription_updated_body("evt_tamper", Utc::now().timestamp(), "cus_other");
            let response = request
                .post("/api/stripe/webhook")
                .add_header("stripe-signature", format!("t={now},v1={signature}"))
                .bytes(tampered.into())
                .await;
            assert_eq!(
                response.status_code(),
                400,
                "response: {:?}",
                response.text()
            );
        })
        .await;
    })
    .await;
}

/// Structural signature failures keep the webhook's plain-text 400 contract.
#[tokio::test]
#[serial(process_environment)]
async fn malformed_signature_headers_have_exact_plain_text_rejections() {
    with_stripe_env(async {
        boot_request::<App, _, _>(|request, ctx| async move {
            require_licence(&ctx);
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
            assert_eq!(invalid.text(), "no v1 signature matched the computed HMAC");

            let malformed = request
                .post("/api/stripe/webhook")
                .add_header(
                    "stripe-signature",
                    format!("t={now},v1={signature},malformed"),
                )
                .bytes(body.clone().into())
                .await;
            assert_eq!(malformed.status_code(), StatusCode::BAD_REQUEST);
            assert_eq!(malformed.text(), "invalid Stripe-Signature header");

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
    })
    .await;
}

/// A valid signature is checked before malformed JSON is reported.
#[tokio::test]
#[serial(process_environment)]
async fn malformed_json_is_rejected_after_signature_verification() {
    with_stripe_env(async {
        boot_request::<App, _, _>(|request, ctx| async move {
            require_licence(&ctx);
            let body = b"not json".to_vec();
            let now = Utc::now().timestamp();
            let valid_signature = sign(WEBHOOK_SECRET, now, &body);

            let valid = request
                .post("/api/stripe/webhook")
                .add_header("stripe-signature", format!("t={now},v1={valid_signature}"))
                .bytes(body.clone().into())
                .await;
            assert_eq!(valid.status_code(), StatusCode::BAD_REQUEST);
            assert_eq!(valid.text(), "invalid JSON body");

            let invalid_signature = request
                .post("/api/stripe/webhook")
                .add_header("stripe-signature", format!("t={now},v1=wrong"))
                .bytes(body.into())
                .await;
            assert_eq!(invalid_signature.status_code(), StatusCode::BAD_REQUEST);
            assert_eq!(
                invalid_signature.text(),
                "no v1 signature matched the computed HMAC"
            );
        })
        .await;
    })
    .await;
}

/// A delayed subscription event is accepted but cannot undo a newer applied event.
#[tokio::test]
#[serial(process_environment)]
async fn a_stale_subscription_event_is_not_reapplied() {
    with_stripe_env(async {
        boot_request::<App, _, _>(|request, ctx| async move {
            require_licence(&ctx);
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
    })
    .await;
}

/// Checkout completion links the Stripe customer so a later subscription event can resolve its tenant.
#[tokio::test]
#[serial(process_environment)]
async fn checkout_completion_links_the_stripe_customer() {
    with_stripe_env(async {
        boot_request::<App, _, _>(|request, ctx| async move {
            require_licence(&ctx);
            let tenant_id = TestTenant::create(&request, &ctx, TenantArgs::default())
                .await
                .tenant_id;
            let body = checkout_completed_body("evt_checkout", 1_000, tenant_id);
            let now = Utc::now().timestamp();
            let subscription =
                subscription_updated_body("evt_before_checkout", 1_001, "cus_checkout");
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
    })
    .await;
}
