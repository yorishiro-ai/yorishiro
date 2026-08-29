use chrono::Utc;
use hmac::{Hmac, Mac};
use loco_rs::testing::prelude::*;
use sea_orm::ActiveValue;
use serde_json::json;
use serial_test::serial;
use sha2::Sha256;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::ee::models::billing;
use yorishiro::models::_entities::identity_tenants;

type HmacSha256 = Hmac<Sha256>;

const WEBHOOK_SECRET: &str = "whsec_test";

/// `StripeConfig::from_env` reads process env vars directly, with no DI seam for it, so tests configure the webhook the same way production does: by setting the vars for the duration of the request.
/// `#[serial]` on every test in this suite makes that safe.
async fn with_stripe_env<T>(fut: impl std::future::Future<Output = T>) -> T {
    // SAFETY: serialized by every test in this binary being #[serial] on the default key.
    unsafe {
        std::env::set_var("YORISHIRO_STRIPE_WEBHOOK_SECRET", WEBHOOK_SECRET);
        std::env::set_var("YORISHIRO_STRIPE_PRICE_PRO", "price_pro");
        std::env::set_var("YORISHIRO_STRIPE_PRICE_TEAM", "price_team");
    }
    let result = fut.await;
    unsafe {
        std::env::remove_var("YORISHIRO_STRIPE_WEBHOOK_SECRET");
        std::env::remove_var("YORISHIRO_STRIPE_PRICE_PRO");
        std::env::remove_var("YORISHIRO_STRIPE_PRICE_TEAM");
    }
    result
}

fn sign(secret: &str, timestamp: i64, payload: &[u8]) -> String {
    let mut signed_payload = format!("{timestamp}.").into_bytes();
    signed_payload.extend_from_slice(payload);
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(&signed_payload);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn create_tenant(conn: &impl sea_orm::ConnectionTrait, name: &str) -> Uuid {
    let active = identity_tenants::ActiveModel {
        name: ActiveValue::Set(name.into()),
        max_workspaces: ActiveValue::Set(None),
        ..Default::default()
    };
    sea_orm::ActiveModelTrait::insert(active, conn)
        .await
        .unwrap()
        .id
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

/// A duplicate delivery of an already-applied event must not re-apply it.
#[tokio::test]
#[serial]
async fn a_duplicate_event_id_is_not_reapplied() {
    with_stripe_env(async {
        request_with_create_db::<App, _, _>(|request, ctx| async move {
            let tenant_id = create_tenant(&ctx.db, "acme").await;
            billing::link_stripe_customer(&ctx.db, tenant_id, "cus_1")
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
            assert_eq!(first.status_code(), 200, "response: {:?}", first.text());
            let after_first = billing::get_billing(&ctx.db, tenant_id)
                .await
                .unwrap()
                .and_then(|r| r.plan);
            assert_eq!(after_first.as_deref(), Some("pro"));

            // Downgrade directly, bypassing the webhook, so a re-application of the duplicate delivery below would be observable as a plan flip back to `pro`.
            billing::set_plan(&ctx.db, tenant_id, "free").await.unwrap();

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
            let after_retry = billing::get_billing(&ctx.db, tenant_id)
                .await
                .unwrap()
                .and_then(|r| r.plan);
            assert_eq!(
                after_retry.as_deref(),
                Some("free"),
                "the duplicate must not have re-applied the plan change"
            );

            super::close_app_pools(&ctx).await;
        })
        .await;
    })
    .await;
}

/// Cancelling a subscription must put the tenant back on Free: both the plan and the workspace cap that comes with it.
#[tokio::test]
#[serial]
async fn a_cancellation_returns_the_tenant_to_free() {
    with_stripe_env(async {
        request_with_create_db::<App, _, _>(|request, ctx| async move {
            let tenant_id = create_tenant(&ctx.db, "acme").await;
            billing::link_stripe_customer(&ctx.db, tenant_id, "cus_cancel")
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
            assert_eq!(up.status_code(), 200, "response: {:?}", up.text());

            let del_body = subscription_deleted_body("evt_del", 2_000, "cus_cancel");
            let now = Utc::now().timestamp();
            let del_sig = sign(WEBHOOK_SECRET, now, &del_body);
            let del = request
                .post("/api/stripe/webhook")
                .add_header("stripe-signature", format!("t={now},v1={del_sig}"))
                .bytes(del_body.into())
                .await;
            assert_eq!(del.status_code(), 200, "response: {:?}", del.text());

            let billing_record = billing::get_billing(&ctx.db, tenant_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(billing_record.plan.as_deref(), Some("free"));

            use sea_orm::EntityTrait;
            let tenant = identity_tenants::Entity::find_by_id(tenant_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                tenant.max_workspaces,
                Some(1),
                "a cancelled tenant must drop to Free's workspace cap, not keep the paid one"
            );

            super::close_app_pools(&ctx).await;
        })
        .await;
    })
    .await;
}

/// With no webhook secret configured there is no way to tell a genuine Stripe delivery from anything else, so the endpoint refuses rather than accepting unverifiable requests.
#[tokio::test]
#[serial]
async fn an_unconfigured_webhook_refuses_rather_than_accepting() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
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

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// A tampered payload (signature computed over a different body) must be rejected.
#[tokio::test]
#[serial]
async fn a_tampered_payload_is_rejected() {
    with_stripe_env(async {
        request_with_create_db::<App, _, _>(|request, ctx| async move {
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

            super::close_app_pools(&ctx).await;
        })
        .await;
    })
    .await;
}
