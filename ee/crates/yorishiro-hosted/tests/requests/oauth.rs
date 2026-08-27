//! Covers what is reachable without a live identity provider: the unconfigured/configured `status` shape, the `authorize`/`callback` unconfigured `404`, an unreachable issuer failing loudly rather than redirecting, `callback`'s rejection paths (provider error, missing `code`/`state`, a badly-signed `state`, an expired `state`), and `find_or_create`'s provisioning rules called directly against `ctx.db` (the tenant cap, matching how `tests/requests/stripe.rs` calls `billing::` functions directly alongside HTTP requests).
//! The redirect `authorize` builds on a reachable issuer, the CSRF cookie it sets, and a full authorization-code round trip through `callback` all need a real or mocked IdP and are not covered here.

use axum::http::header;
use hmac::{Hmac, Mac};
use loco_rs::testing::prelude::*;
use sea_orm::{ActiveValue, EntityTrait};
use serial_test::serial;
use sha2::Sha256;
use yorishiro_core::models::_entities::identity_tenants;
use yorishiro_hosted::HostedApp;
use yorishiro_hosted::services::oauth;

/// A loopback address nothing listens on, so `authorize`'s discovery fetch fails fast with a connection refusal rather than depending on real DNS/network reachability in CI.
/// Loopback, not a public hostname, so `discovery::require_https_or_loopback`'s `http://` exemption applies without needing a TLS-terminating stub server.
const ISSUER_URL: &str = "http://127.0.0.1:1";
const CLIENT_ID: &str = "test-client";
const CLIENT_SECRET: &str = "test-client-secret";

/// A signed `state` value in the exact shape `services::oauth::state_token::issue` produces: this crate keeps that module private, so a test that needs a validly-signed fixture (rather than exercising the rejection paths, which need no valid signature at all) builds one the same way, HMAC-SHA256 under the client secret.
fn sign_state(payload: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(CLIENT_SECRET.as_bytes()).expect("any key length is valid");
    mac.update(payload.as_bytes());
    let signature = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("{payload}.{signature}")
}

/// `OAuthConfig::from_env` reads process env vars directly on every request (this branch has no DI seam for it, see `controllers::oauth`'s module doc comment), so tests configure OAuth the same way production does: by setting the vars for the duration of the request.
/// `#[serial]` on every test in this suite makes that safe.
async fn with_oauth_env<T>(fut: impl std::future::Future<Output = T>) -> T {
    // SAFETY: serialized by every test in this binary being #[serial] on the default key.
    unsafe {
        std::env::set_var("YORISHIRO_OAUTH_ISSUER_URL", ISSUER_URL);
        std::env::set_var("YORISHIRO_OAUTH_CLIENT_ID", CLIENT_ID);
        std::env::set_var("YORISHIRO_OAUTH_CLIENT_SECRET", CLIENT_SECRET);
    }
    let result = fut.await;
    unsafe {
        std::env::remove_var("YORISHIRO_OAUTH_ISSUER_URL");
        std::env::remove_var("YORISHIRO_OAUTH_CLIENT_ID");
        std::env::remove_var("YORISHIRO_OAUTH_CLIENT_SECRET");
    }
    result
}

/// `GET /auth/oauth/status` always answers 200, whether or not OAuth is configured: a client deciding whether to show the "Sign in with SSO" button has no other way to tell "not configured" apart from "not present".
#[tokio::test]
#[serial]
async fn status_reports_disabled_when_unconfigured_and_enabled_when_configured() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        let disabled = request.get("/auth/oauth/status").await;
        assert_eq!(disabled.status_code(), 200);
        assert_eq!(
            disabled.json::<serde_json::Value>(),
            serde_json::json!({ "enabled": false })
        );

        with_oauth_env(async {
            let enabled = request.get("/auth/oauth/status").await;
            assert_eq!(enabled.status_code(), 200);
            assert_eq!(
                enabled.json::<serde_json::Value>(),
                serde_json::json!({ "enabled": true })
            );
        })
        .await;

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// An issuer set with no `client_id`/`client_secret` is a misconfiguration, not "unconfigured": `OAuthConfig::from_env` used to panic here (it read once at boot, so a panic was a fail-fast startup failure); read per-request instead, it must report `500` rather than crash the task handling the request, and must not be silently treated as `enabled: false`.
#[tokio::test]
#[serial]
async fn status_errors_loudly_when_partially_configured() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        // SAFETY: serialized by every test in this binary being #[serial] on the default key.
        unsafe {
            std::env::set_var("YORISHIRO_OAUTH_ISSUER_URL", "https://idp.example.com");
        }
        let response = request.get("/auth/oauth/status").await;
        unsafe {
            std::env::remove_var("YORISHIRO_OAUTH_ISSUER_URL");
        }

        assert_eq!(
            response.status_code(),
            500,
            "a set issuer with no client_id/client_secret must error, not panic or read as \
             disabled: {:?}",
            response.text()
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// With no `YORISHIRO_OAUTH_ISSUER_URL` set, `authorize` and `callback` must answer exactly the same `404` a route that does not exist would: a prober must not be able to tell "unconfigured" apart from "absent".
#[tokio::test]
#[serial]
async fn authorize_and_callback_404_when_unconfigured() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        let authorize = request.get("/auth/oauth/authorize").await;
        assert_eq!(
            authorize.status_code(),
            404,
            "response: {:?}",
            authorize.text()
        );

        let callback = request.get("/auth/oauth/callback?code=abc&state=xyz").await;
        assert_eq!(
            callback.status_code(),
            404,
            "response: {:?}",
            callback.text()
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// `authorize` reaches the identity provider to fetch its discovery document before it can redirect anywhere, so a configured but unreachable issuer fails loudly with an internal error rather than a redirect to nowhere.
/// Exercising the redirect itself, and the CSRF cookie it sets, needs a live IdP and is out of scope here (see the module doc comment).
#[tokio::test]
#[serial]
async fn authorize_fails_loudly_against_an_unreachable_issuer() {
    with_oauth_env(async {
        request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
            let response = request.get("/auth/oauth/authorize").await;
            assert_eq!(
                response.status_code(),
                500,
                "an unreachable issuer must fail loudly, never redirect: {:?}",
                response.text()
            );

            super::close_app_pools(&ctx).await;
        })
        .await;
    })
    .await;
}

/// The provider itself reporting an error (the user declined consent, an app misconfiguration, ...) redirects to the login page's failure state rather than surfacing raw JSON to a browser mid-redirect, and clears the CSRF cookie so it is never reusable across two attempts.
#[tokio::test]
#[serial]
async fn callback_redirects_to_login_failure_when_the_provider_reports_an_error() {
    with_oauth_env(async {
        request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
            let response = request
                .get("/auth/oauth/callback?error=access_denied&error_description=user+declined")
                .await;

            assert_eq!(
                response.status_code(),
                302,
                "response: {:?}",
                response.text()
            );
            assert_eq!(
                response.header(header::LOCATION),
                "/#/login?error=oauth_failed"
            );
            assert!(
                response
                    .header(header::SET_COOKIE)
                    .to_str()
                    .unwrap()
                    .contains("Max-Age=0"),
                "the CSRF cookie must be cleared on this path too"
            );

            super::close_app_pools(&ctx).await;
        })
        .await;
    })
    .await;
}

/// A callback missing `code` or `state` (hit directly, without ever having visited `authorize`) redirects to the same login-failure state rather than a raw validation error.
#[tokio::test]
#[serial]
async fn callback_redirects_to_login_failure_when_code_or_state_is_missing() {
    with_oauth_env(async {
        request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
            let missing_state = request.get("/auth/oauth/callback?code=abc").await;
            assert_eq!(missing_state.status_code(), 302);
            assert_eq!(
                missing_state.header(header::LOCATION),
                "/#/login?error=oauth_failed"
            );

            let missing_code = request.get("/auth/oauth/callback?state=xyz").await;
            assert_eq!(missing_code.status_code(), 302);
            assert_eq!(
                missing_code.header(header::LOCATION),
                "/#/login?error=oauth_failed"
            );

            super::close_app_pools(&ctx).await;
        })
        .await;
    })
    .await;
}

/// A forged `state` (never issued by this process, so its HMAC signature cannot verify) is rejected with `401` before any provider is contacted: `handle_callback` checks `state` first.
#[tokio::test]
#[serial]
async fn callback_rejects_a_state_with_a_bad_signature() {
    with_oauth_env(async {
        request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
            let response = request
                .get("/auth/oauth/callback?code=abc&state=1234567890.deadbeef.verifier.notasignature")
                .await;

            assert_eq!(response.status_code(), 401, "response: {:?}", response.text());

            super::close_app_pools(&ctx).await;
        })
        .await;
    })
    .await;
}

/// A `state` value signed correctly (under the deployment's own client secret, so the HMAC check that `callback_rejects_a_state_with_a_bad_signature` covers passes) but too old (older than `STATE_TTL_SECS`) is rejected the same `401` way: `state_token::verify`'s expiry check runs before the CSRF cookie is ever consulted.
#[tokio::test]
#[serial]
async fn callback_rejects_an_expired_state() {
    with_oauth_env(async {
        request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
            let issued_at = chrono::Utc::now().timestamp() - 700; // past STATE_TTL_SECS (600)
            let state = sign_state(&format!("{issued_at}.deadbeef.verifier"));

            let response = request
                .get(&format!("/auth/oauth/callback?code=abc&state={state}"))
                .await;

            assert_eq!(
                response.status_code(),
                401,
                "response: {:?}",
                response.text()
            );

            super::close_app_pools(&ctx).await;
        })
        .await;
    })
    .await;
}

/// `find_or_create` must not be a backdoor around `YORISHIRO_MAX_TENANTS`: with the cap already met, a brand-new identity (one `find_by_oauth_identity` finds nothing for) is refused rather than silently given a fresh tenant.
/// Called directly against `ctx.db` rather than through `callback`, since driving this via HTTP would need a live IdP.
#[tokio::test]
#[serial]
async fn find_or_create_refuses_a_new_tenant_past_the_cap() {
    request_with_create_db::<HostedApp, _, _>(|_request, ctx| async move {
        let existing_tenant = identity_tenants::ActiveModel {
            name: ActiveValue::Set("existing".into()),
            ..Default::default()
        };
        sea_orm::ActiveModelTrait::insert(existing_tenant, &ctx.db)
            .await
            .unwrap();

        // SAFETY: serialized by every test in this binary being #[serial] on the default key.
        unsafe {
            std::env::set_var("YORISHIRO_MAX_TENANTS", "1");
        }
        let result = oauth::find_or_create(
            &ctx.db,
            "oidc",
            "a-brand-new-subject",
            Some("newcomer@example.com"),
            None,
            ("test-model", 768),
        )
        .await;
        unsafe {
            std::env::remove_var("YORISHIRO_MAX_TENANTS");
        }

        match result {
            Err(yorishiro_core::YorishiroError::ScopeInsufficient { message, .. }) => {
                assert!(
                    message.contains("tenant limit"),
                    "wrong ScopeInsufficient message: {message}"
                );
            }
            _ => panic!(
                "a new identity past the cap must be refused with ScopeInsufficient naming the \
                 tenant limit"
            ),
        }

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// A first login must not leave the auto-provisioned workspace `schema_pending`: there is no admin present afterward to choose a starting schema for it, unlike `controllers::setup::setup`'s own bootstrap workspace.
/// The workspace is provisioned with a schema from the built-in `general-notes` template and left `active`.
#[tokio::test]
#[serial]
async fn find_or_create_provisions_an_active_workspace_with_a_general_notes_schema() {
    request_with_create_db::<HostedApp, _, _>(|_request, ctx| async move {
        let provisioned = oauth::find_or_create(
            &ctx.db,
            "oidc",
            "a-first-login-subject",
            Some("firstlogin@example.com"),
            None,
            ("test-model", 768),
        )
        .await
        .expect("first login provisioning");

        let workspace = yorishiro_core::models::_entities::identity_workspaces::Entity::find_by_id(
            provisioned.workspace_id,
        )
        .one(&ctx.db)
        .await
        .unwrap()
        .expect("the provisioned workspace must exist");

        assert_eq!(
            workspace.status,
            yorishiro_core::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE,
            "an auto-provisioned workspace must not be left schema_pending"
        );
        let schema_id = workspace
            .schema_id
            .expect("the workspace must have a schema linked");

        let schema =
            yorishiro_core::models::_entities::content_schemas::Entity::find_by_id(schema_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .expect("the linked schema must exist");
        assert_eq!(schema.name, "general-notes");

        super::close_app_pools(&ctx).await;
    })
    .await;
}
