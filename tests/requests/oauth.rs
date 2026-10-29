//! Covers what is reachable without a live identity provider: the unconfigured/configured `status` shape, the `authorize`/`callback` unconfigured `404`, an unreachable issuer failing loudly rather than redirecting, `callback`'s rejection paths (provider error, missing `code`/`state`, a badly-signed `state`, an expired `state`), and `find_or_create`'s provisioning rules called directly against `ctx.db` (the tenant cap, matching how `tests/requests/stripe.rs` calls `tenant_billing::` functions directly alongside HTTP requests).
//! The redirect `authorize` builds on a reachable issuer, the CSRF cookie it sets, and a full authorization-code round trip through `callback` all need a real or mocked IdP and are not covered here.

use super::boot_request;
use axum::http::StatusCode;
use axum::http::header;
use sea_orm::{EntityTrait, TransactionTrait};
use serial_test::serial;
use yorishiro::App;
use yorishiro::db::AppContextBackend;
use yorishiro::edition::ee::controllers::middleware::edition::LicenceState;

fn require_licence(ctx: &loco_rs::app::AppContext) {
    assert!(
        ctx.shared_store
            .get::<std::sync::Arc<LicenceState>>()
            .expect("production boot installs licence state")
            .is_active(),
        "licensed OAuth tests require a valid YORISHIRO_LICENSE_KEY at boot"
    );
}

/// A loopback address nothing listens on, so `authorize`'s discovery fetch fails fast with a connection refusal rather than depending on real DNS/network reachability in CI.
/// Loopback, not a public hostname, so `discovery::require_https_or_loopback`'s `http://` exemption applies without needing a TLS-terminating stub server.
const ISSUER_URL: &str = "http://127.0.0.1:1";
const CLIENT_ID: &str = "test-client";
const CLIENT_SECRET: &str = "test-client-secret";

#[tokio::test]
#[serial(process_environment)]
async fn invitation_callback_verifies_id_token_and_rejects_cookie_tampering() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, PaginatorTrait, QueryFilter};
    use std::sync::{Arc, Mutex};
    use yorishiro::models::tenant_memberships::MembershipRole;
    use yorishiro::models::{user_users, workspace_invites};

    let directory = tempfile::tempdir().unwrap();
    let key_path = directory.path().join("oauth-test.pem");
    assert!(
        std::process::Command::new("openssl")
            .args(["genrsa", "-out"])
            .arg(&key_path)
            .arg("2048")
            .status()
            .unwrap()
            .success()
    );
    let modulus = std::process::Command::new("openssl")
        .arg("rsa")
        .arg("-in")
        .arg(&key_path)
        .args(["-modulus", "-noout"])
        .output()
        .unwrap();
    assert!(modulus.status.success());
    let hex = String::from_utf8(modulus.stdout).unwrap();
    let hex = hex.trim().strip_prefix("Modulus=").unwrap();
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
        .collect::<Vec<_>>();
    let jwks = serde_json::json!({"keys":[{"kty":"RSA","kid":"invite-test","use":"sig","alg":"RS256","n":URL_SAFE_NO_PAD.encode(bytes),"e":"AQAB"}]});
    let signing_key =
        jsonwebtoken::EncodingKey::from_rsa_pem(&std::fs::read(key_path).unwrap()).unwrap();
    let subject = uuid::Uuid::new_v4().to_string();
    let issuer = Arc::new(Mutex::new(String::new()));
    let discovery_issuer = issuer.clone();
    let token_issuer = issuer.clone();
    let token_subject = subject.clone();
    let stub = crate::http_stub::HttpStub::start(axum::Router::new()
        .route("/.well-known/openid-configuration", axum::routing::get(move || {
            let issuer = discovery_issuer.lock().unwrap().clone();
            async move { axum::Json(serde_json::json!({"authorization_endpoint":format!("{issuer}/authorize"),"token_endpoint":format!("{issuer}/token"),"jwks_uri":format!("{issuer}/jwks")})) }
        }))
        .route("/jwks", axum::routing::get(move || { let jwks = jwks.clone(); async move { axum::Json(jwks) } }))
        .route("/token", axum::routing::post(move |axum::Form(form): axum::Form<std::collections::HashMap<String,String>>| {
            assert_eq!(form["grant_type"], "authorization_code");
            assert_eq!(form["client_id"], CLIENT_ID);
            assert_eq!(form["client_secret"], CLIENT_SECRET);
            assert_eq!(form["code_verifier"].len(), 43);
            let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
            header.kid = Some("invite-test".into());
            let subject = if form["code"] == "unverified-email" { "unknown-unverified-subject" } else { &token_subject };
            let token = jsonwebtoken::encode(&header, &serde_json::json!({"sub":subject,"email":"unverified@example.com","email_verified":false,"iss":token_issuer.lock().unwrap().clone(),"aud":CLIENT_ID,"exp":chrono::Utc::now().timestamp()+300}), &signing_key).unwrap();
            async move { axum::Json(serde_json::json!({"id_token":token})) }
        }))).await;
    *issuer.lock().unwrap() = stub.url.clone();
    let guard = crate::EnvGuard::capture(&OAUTH_VARS);
    guard.set(crate::env::TestEnv::OauthIssuerUrl.name(), &stub.url);
    guard.set(crate::env::TestEnv::OauthClientId.name(), CLIENT_ID);
    guard.set(crate::env::TestEnv::OauthClientSecret.name(), CLIENT_SECRET);
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let start = request.get("/auth/oauth/authorize").await;
        let url = reqwest::Url::parse(
            start
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let csrf = start
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap())
            .find(|value| value.starts_with("ysr_oauth_csrf="))
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        request
            .get("/auth/oauth/callback")
            .add_query_param("code", "unverified-email")
            .add_query_param("state", &state)
            .add_header(header::COOKIE, csrf)
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            user_users::Entity::find()
                .filter(
                    yorishiro::models::_entities::user_users::Column::Email
                        .eq("unverified@example.com")
                )
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none()
        );
        let tenant = super::fixtures::TestTenant::create(
            &request,
            &ctx,
            super::fixtures::TenantArgs::default(),
        )
        .await;
        let user = user_users::ActiveModel {
            email: Set(super::fixtures::unique_email("oauth-callback-invite")),
            oauth_provider: Set(Some("oidc".into())),
            oauth_subject_id: Set(Some(subject)),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        let (invite, token) = workspace_invites::create_invite(
            &ctx.db,
            tenant.tenant_id,
            &user.email,
            MembershipRole::Viewer,
            chrono::Duration::hours(1),
        )
        .await
        .unwrap();
        let start = request
            .post("/auth/oauth/invites/accept")
            .json(&serde_json::json!({"invite_token":token}))
            .await;
        assert_eq!(start.status_code(), StatusCode::SEE_OTHER);
        let url = reqwest::Url::parse(
            start
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let cookies = start
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|cookie| {
                cookie
                    .to_str()
                    .unwrap()
                    .split(';')
                    .next()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        let cookie_header = cookies.join("; ");
        let mut tampered = cookies.clone();
        let cookie = tampered
            .iter_mut()
            .find(|cookie| cookie.starts_with("ysr_oauth_invite="))
            .unwrap();
        cookie.push('0');
        request
            .get("/auth/oauth/callback")
            .add_query_param("code", "test-code")
            .add_query_param("state", &state)
            .add_header(header::COOKIE, tampered.join("; "))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        assert!(
            workspace_invites::Entity::find_by_id(invite.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .used_at
                .is_none()
        );
        let response = request
            .get("/auth/oauth/callback")
            .add_query_param("code", "test-code")
            .add_query_param("state", &state)
            .add_header(header::COOKIE, cookie_header.clone())
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::FOUND,
            "{}",
            response.text()
        );
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/#/login?invite=accepted"
        );
        let cleared = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(cleared.len(), 2);
        assert!(
            cleared
                .iter()
                .all(|cookie| cookie.to_str().unwrap().contains("Max-Age=0"))
        );
        assert!(
            workspace_invites::Entity::find_by_id(invite.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .used_at
                .is_some()
        );
        use yorishiro::models::_entities::tenant_memberships;
        let membership = tenant_memberships::Entity::find()
            .filter(tenant_memberships::Column::UserId.eq(user.id))
            .filter(tenant_memberships::Column::TenantId.eq(tenant.tenant_id))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(membership.role, "viewer");
        use yorishiro::models::_entities::api_keys;
        assert_eq!(
            api_keys::Entity::find()
                .filter(api_keys::Column::UserId.eq(user.id))
                .count(&ctx.db)
                .await
                .unwrap(),
            0
        );
        assert!(
            yorishiro::models::tenancy::list_workspaces_for_user(&ctx.db, user.id)
                .await
                .unwrap()
                .is_empty()
        );
        request
            .get("/auth/oauth/callback")
            .add_query_param("code", "test-code")
            .add_query_param("state", &state)
            .add_header(header::COOKIE, cookie_header)
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    })
    .await;
}

#[tokio::test]
#[serial(process_environment)]
async fn invite_authorization_keeps_token_out_of_redirect_and_binds_cookies() {
    let stub = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/.well-known/openid-configuration",
        axum::routing::get(|| async {
            axum::Json(serde_json::json!({
                "authorization_endpoint":"https://idp.example.com/authorize",
                "token_endpoint":"https://idp.example.com/token",
                "jwks_uri":"https://idp.example.com/jwks"
            }))
        }),
    ))
    .await;
    let guard = crate::EnvGuard::capture(&OAUTH_VARS);
    guard.set(crate::env::TestEnv::OauthIssuerUrl.name(), &stub.url);
    guard.set(crate::env::TestEnv::OauthClientId.name(), CLIENT_ID);
    guard.set(crate::env::TestEnv::OauthClientSecret.name(), CLIENT_SECRET);
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        request
            .post("/auth/oauth/invites/accept")
            .json(&serde_json::json!({"invite_token":"invalid"}))
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        let token = "a".repeat(48);
        let response = request
            .post("/auth/oauth/invites/accept")
            .json(&serde_json::json!({"invite_token":token}))
            .await;
        assert_eq!(response.status_code(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(!location.contains(&token));
        let url = reqwest::Url::parse(location).unwrap();
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(cookies.len(), 2);
        for cookie in &cookies {
            assert!(
                cookie.contains("HttpOnly; SameSite=Lax; Path=/auth/oauth/callback; Max-Age=600")
            );
        }
        let csrf = cookies
            .iter()
            .find(|value| value.starts_with("ysr_oauth_csrf="))
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .split_once('=')
            .unwrap()
            .1;
        let invite = cookies
            .iter()
            .find(|value| value.starts_with("ysr_oauth_invite="))
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .split_once('=')
            .unwrap()
            .1;
        let (payload, signature) = invite.rsplit_once('.').unwrap();
        assert_eq!(payload, format!("{csrf}.{token}"));
        assert!(yorishiro::edition::ee::controllers::hmac_sign::verify(
            CLIENT_SECRET.as_bytes(),
            payload.as_bytes(),
            signature
        ));
        let verified = yorishiro::edition::ee::controllers::oauth::state_token::verify(
            CLIENT_SECRET.as_bytes(),
            &state,
        )
        .unwrap();
        assert_eq!(
            verified.csrf_hash,
            yorishiro::edition::ee::controllers::oauth::state_token::hash_csrf_cookie(csrf)
        );
        request
            .get("/auth/oauth/callback")
            .add_query_param("code", "unused")
            .add_query_param("state", &state)
            .add_header(
                header::COOKIE,
                format!("ysr_oauth_csrf=wrong; ysr_oauth_invite={invite}"),
            )
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    })
    .await;
}

#[tokio::test]
async fn verified_subject_accepts_invite_without_workspace_or_key() {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    use yorishiro::edition::ee::{
        models::user_users as oauth_users, services::oauth::CallbackIdentity,
    };
    use yorishiro::models::tenant_memberships::MembershipRole;
    use yorishiro::models::{user_users, workspace_invites};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = super::fixtures::TestTenant::create(
            &request,
            &ctx,
            super::fixtures::TenantArgs::default(),
        )
        .await;
        let subject = uuid::Uuid::new_v4().to_string();
        let user = user_users::ActiveModel {
            email: Set(super::fixtures::unique_email("oauth-invite-only")),
            oauth_provider: Set(Some("oidc".into())),
            oauth_subject_id: Set(Some(subject.clone())),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        let (invite, token) = workspace_invites::create_invite(
            &ctx.db,
            tenant.tenant_id,
            &user.email,
            MembershipRole::Viewer,
            chrono::Duration::hours(1),
        )
        .await
        .unwrap();
        let mut identity = CallbackIdentity {
            subject_id: uuid::Uuid::new_v4().to_string(),
            email: Some(user.email.clone()),
            display_name: None,
        };
        assert!(matches!(
            oauth_users::accept_invite(&ctx.db, &identity, &token).await,
            Err(yorishiro::YorishiroError::Unauthenticated)
        ));
        assert!(
            workspace_invites::Entity::find_by_id(invite.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .used_at
                .is_none()
        );
        identity.subject_id = subject;
        // The verified subject, not a mutable provider email claim, selects the stored account.
        identity.email = Some("unrelated@example.com".into());
        let (accepted, redeemed) = oauth_users::accept_invite(&ctx.db, &identity, &token)
            .await
            .unwrap();
        assert_eq!(accepted.id, user.id);
        assert_eq!(redeemed.tenant_id, tenant.tenant_id);
        assert!(accepted.password_hash.is_none());
        assert!(
            yorishiro::models::tenancy::list_workspaces_for_user(&ctx.db, user.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            oauth_users::accept_invite(&ctx.db, &identity, &token)
                .await
                .is_err()
        );
    })
    .await;
}

const OAUTH_VARS: [&str; 3] = [
    crate::env::TestEnv::OauthIssuerUrl.name(),
    crate::env::TestEnv::OauthClientId.name(),
    crate::env::TestEnv::OauthClientSecret.name(),
];

/// The shipped Loco configuration interpolates these values before each app boots.
/// Callers that change these variables take `serial(process_environment)`.
async fn with_oauth_env<T>(fut: impl std::future::Future<Output = T>) -> T {
    let guard = crate::EnvGuard::capture(&OAUTH_VARS);
    guard.set(crate::env::TestEnv::OauthIssuerUrl.name(), ISSUER_URL);
    guard.set(crate::env::TestEnv::OauthClientId.name(), CLIENT_ID);
    guard.set(crate::env::TestEnv::OauthClientSecret.name(), CLIENT_SECRET);
    fut.await
}
/// `GET /auth/oauth/status` answers 200 in both states this test sets, unconfigured and fully configured, reporting the difference in `enabled` rather than in the status code: a client deciding whether to show the "Sign in with SSO" button has no other way to tell "not configured" apart from "not present".
/// The third state, an issuer set with no `client_id`/`client_secret`, answers 500 instead and belongs to `status_errors_loudly_when_partially_configured` below.
#[tokio::test]
#[serial(process_environment)]
async fn status_reports_disabled_when_unconfigured_and_enabled_when_configured() {
    let ambient = crate::EnvGuard::capture(&OAUTH_VARS);
    for variable in OAUTH_VARS {
        ambient.remove(variable);
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let disabled = request.get("/auth/oauth/status").await;
        assert_eq!(disabled.status_code(), StatusCode::OK);
        assert_eq!(
            disabled.json::<serde_json::Value>(),
            serde_json::json!({ "enabled": false })
        );

        with_oauth_env(async {
            let unchanged = request.get("/auth/oauth/status").await;
            assert_eq!(
                unchanged.json::<serde_json::Value>(),
                serde_json::json!({"enabled": false})
            );
            boot_request::<App, _, _>(|request, ctx| async move {
                require_licence(&ctx);
                let enabled = request.get("/auth/oauth/status").await;
                assert_eq!(enabled.status_code(), StatusCode::OK);
                assert_eq!(
                    enabled.json::<serde_json::Value>(),
                    serde_json::json!({ "enabled": true })
                );
            })
            .await;
        })
        .await;
    })
    .await;
}

/// Partial resolved OAuth configuration must report 500 rather than panic or silently disable login.
#[tokio::test]
#[serial(process_environment)]
async fn status_errors_loudly_when_partially_configured() {
    let guard = crate::EnvGuard::capture(&OAUTH_VARS);
    guard.remove(crate::env::TestEnv::OauthClientId.name());
    guard.remove(crate::env::TestEnv::OauthClientSecret.name());
    guard.set(
        crate::env::TestEnv::OauthIssuerUrl.name(),
        "https://idp.example.com",
    );
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let response = request.get("/auth/oauth/status").await;

        assert_eq!(
            response.status_code(),
            500,
            "a set issuer with no client_id/client_secret must error, not panic or read as \
             disabled: {:?}",
            response.text()
        );
    })
    .await;
}

/// With no `YORISHIRO_OAUTH_ISSUER_URL` set, `authorize` and `callback` must answer exactly the same `404` a route that does not exist would: a prober must not be able to tell "unconfigured" apart from "absent".
#[tokio::test]
#[serial(process_environment)]
async fn authorize_and_callback_404_when_unconfigured() {
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
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
    })
    .await;
}

/// `authorize` reaches the identity provider to fetch its discovery document before it can redirect anywhere, so a configured but unreachable issuer fails loudly with an internal error rather than a redirect to nowhere.
/// Exercising the redirect itself, and the CSRF cookie it sets, needs a live IdP and is out of scope here (see the module doc comment).
#[tokio::test]
#[serial(process_environment)]
async fn authorize_fails_loudly_against_an_unreachable_issuer() {
    with_oauth_env(async {
        boot_request::<App, _, _>(|request, ctx| async move {
            require_licence(&ctx);
            let response = request.get("/auth/oauth/authorize").await;
            assert_eq!(
                response.status_code(),
                500,
                "an unreachable issuer must fail loudly, never redirect: {:?}",
                response.text()
            );
        })
        .await;
    })
    .await;
}

/// The provider itself reporting an error (the user declined consent, an app misconfiguration, ...) redirects to the login page's failure state rather than surfacing raw JSON to a browser mid-redirect, and clears the CSRF cookie so it is never reusable across two attempts.
#[tokio::test]
#[serial(process_environment)]
async fn callback_redirects_to_login_failure_when_the_provider_reports_an_error() {
    with_oauth_env(async {
        boot_request::<App, _, _>(|request, ctx| async move {
            require_licence(&ctx);
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
        })
        .await;
    })
    .await;
}

/// A callback missing `code` or `state` (hit directly, without ever having visited `authorize`) redirects to the same login-failure state rather than a raw validation error.
#[tokio::test]
#[serial(process_environment)]
async fn callback_redirects_to_login_failure_when_code_or_state_is_missing() {
    with_oauth_env(async {
        boot_request::<App, _, _>(|request, ctx| async move {
            require_licence(&ctx);
            let missing_state = request.get("/auth/oauth/callback?code=abc").await;
            assert_eq!(missing_state.status_code(), StatusCode::FOUND);
            assert_eq!(
                missing_state.header(header::LOCATION),
                "/#/login?error=oauth_failed"
            );

            let missing_code = request.get("/auth/oauth/callback?state=xyz").await;
            assert_eq!(missing_code.status_code(), StatusCode::FOUND);
            assert_eq!(
                missing_code.header(header::LOCATION),
                "/#/login?error=oauth_failed"
            );
        })
        .await;
    })
    .await;
}

/// A forged `state` (never issued by this process, so its HMAC signature cannot verify) is rejected with `401` before any provider is contacted: `handle_callback` checks `state` first.
#[tokio::test]
#[serial(process_environment)]
async fn callback_rejects_a_state_with_a_bad_signature() {
    with_oauth_env(async {
        boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
            let response = request
                .get("/auth/oauth/callback?code=abc&state=1234567890.deadbeef.verifier.notasignature")
                .await;

            assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED, "response: {:?}", response.text());

        })
        .await;
    })
    .await;
}

/// A `state` value signed correctly (under the deployment's own client secret, so the HMAC check that `callback_rejects_a_state_with_a_bad_signature` covers passes) but too old (older than `STATE_TTL_SECS`) is rejected the same `401` way: `state_token::verify`'s expiry check runs before the CSRF cookie is ever consulted.
#[tokio::test]
#[serial(process_environment)]
async fn callback_rejects_an_expired_state() {
    with_oauth_env(async {
        boot_request::<App, _, _>(|request, ctx| async move {
            require_licence(&ctx);
            let issued_at = chrono::Utc::now().timestamp() - 700; // past STATE_TTL_SECS (600)
            let state = yorishiro::edition::ee::controllers::oauth::state_token::issue_at(
                CLIENT_SECRET.as_bytes(),
                issued_at,
            )
            .state;

            let response = request
                .get(&format!("/auth/oauth/callback?code=abc&state={state}"))
                .await;

            assert_eq!(
                response.status_code(),
                401,
                "response: {:?}",
                response.text()
            );
        })
        .await;
    })
    .await;
}

/// `find_or_create` must not be a backdoor around `YORISHIRO_MAX_TENANTS`: with the cap already met, a brand-new identity (one `find_by_oauth_identity` finds nothing for) is refused rather than silently given a fresh tenant.
/// The migration's user_users_oauth_identity_idx is partial (oauth_provider IS NOT NULL).
/// Fresh generation with the lockfile-pinned CLI omits this partial index entirely; no application/test path uses Schema::create_table_from_entity or other entity-based schema creation, so the migration is its only owner.
/// Called directly against `ctx.db` rather than through `callback`, since driving this via HTTP would need a live IdP.
#[tokio::test]
async fn find_or_create_refuses_a_new_tenant_past_the_cap() {
    super::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        super::fixtures::TestTenant::create(&request, &ctx, super::fixtures::TenantArgs::default())
            .await;

        // `find_or_create` takes a transaction because the advisory locks it and `create_workspace` rely on are transaction-scoped, which is also how `controllers::oauth` calls it.
        for max_tenants in [Some(1), None] {
            if max_tenants.is_none() && !ctx.is_sqlite() {
                continue;
            }
            let txn = ctx.db.begin().await.expect("begin");
            let result = yorishiro::edition::ee::models::user_users::find_or_create(
                &txn,
                "oidc",
                "a-brand-new-subject",
                Some("newcomer@example.com"),
                None,
                max_tenants,
                false,
            )
            .await;
            // The call is expected to be refused; make the rollback explicit so a
            // future implementation that retains the transaction cannot leak it.
            txn.rollback().await.expect("rollback refused provisioning");
            match result {
                Err(yorishiro::YorishiroError::Conflict { message }) => {
                    assert!(
                        message.contains("tenant limit"),
                        "wrong Conflict message: {message}"
                    );
                }
                _ => panic!(
                    "a new identity past the cap must be refused with Conflict naming the \
                 tenant limit"
                ),
            }
        }
    })
    .await;
}

/// A first login must not leave the auto-provisioned workspace `schema_pending`: there is no admin present afterward to choose a starting schema for it, unlike `controllers::setup::setup`'s own bootstrap workspace.
/// The workspace is provisioned with a schema from the built-in `general-notes` template and left `active`.
#[tokio::test]
async fn find_or_create_provisions_an_active_workspace_with_a_general_notes_schema() {
    super::boot_request_isolated::<App, _, _>(|_request, ctx| async move {
        // `find_or_create` takes a transaction because the advisory locks it and `create_workspace` rely on are transaction-scoped, which is also how `controllers::oauth` calls it.
        let txn = ctx.db.begin().await.expect("begin");
        let provisioned = yorishiro::edition::ee::models::user_users::find_or_create(
            &txn,
            "oidc",
            "a-first-login-subject",
            Some("firstlogin@example.com"),
            None,
            Some(1),
            false,
        )
        .await
        .expect("first login provisioning");
        // The assertions below read these rows back through `ctx.db`, so an uncommitted transaction would leave them invisible.
        txn.commit().await.expect("commit provisioning");

        let workspace = yorishiro::models::_entities::workspace_workspaces::Entity::find_by_id(
            provisioned.workspace_id,
        )
        .one(&ctx.db)
        .await
        .unwrap()
        .expect("the provisioned workspace must exist");

        assert_eq!(
            workspace.status,
            yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE,
            "an auto-provisioned workspace must not be left schema_pending"
        );
        let schema_id = workspace
            .schema_id
            .expect("the workspace must have a schema linked");

        let schema = yorishiro::models::schema_schemas::get_active_schema(
            &ctx.db,
            provisioned.workspace_id,
            "general-notes",
        )
        .await
        .expect("the linked schema must be active");
        assert_eq!(schema.id, schema_id);
        assert_eq!(schema.name, "general-notes");
        use sea_orm::{ColumnTrait, QueryFilter};
        use yorishiro::models::_entities::tenant_memberships;
        let memberships = tenant_memberships::Entity::find()
            .filter(tenant_memberships::Column::TenantId.eq(workspace.tenant_id))
            .all(&ctx.db)
            .await
            .expect("OAuth tenant owner");
        assert_eq!(memberships.len(), 1);
        assert_eq!(memberships[0].role, "owner");
        let access = yorishiro::models::access::workspace_access(
            &ctx.db,
            workspace.tenant_id,
            workspace.id,
            memberships[0].user_id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            access.max_scope,
            yorishiro::models::api_keys::ApiKeyScope::Migration
        );
        let candidates =
            yorishiro::models::tenancy::list_workspaces_for_user(&ctx.db, memberships[0].user_id)
                .await
                .expect("OAuth workspace discovery");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, workspace.id);
    })
    .await;
}

#[tokio::test]
#[serial(process_environment)]
async fn oauth_provisioning_policy_controls_new_caps() {
    let guard = crate::EnvGuard::capture(&[crate::env::TestEnv::PlanPolicyEnabled.name()]);
    for (index, value, expected_tenant, expected_entities) in
        [(0, None, None, None), (1, Some("true"), Some(1), Some(500))]
    {
        match value {
            Some(v) => guard.set(crate::env::TestEnv::PlanPolicyEnabled.name(), v),
            None => guard.remove(crate::env::TestEnv::PlanPolicyEnabled.name()),
        }
        super::boot_request_isolated::<App, _, _>(|_request, ctx| async move {
            let policy_enabled =
                yorishiro::edition::ee::data::plan::plan_policy_enabled(&ctx.config)
                    .expect("resolve provisioning policy");
            let txn = ctx.db.begin().await.expect("begin");
            let provisioned = yorishiro::edition::ee::models::user_users::find_or_create(
                &txn,
                "oidc",
                &format!("policy-subject-{index}"),
                Some(&format!("policy-{index}@example.com")),
                None,
                Some(1),
                policy_enabled,
            )
            .await
            .expect("provisioning");
            txn.commit().await.expect("commit");
            let workspace = yorishiro::models::_entities::workspace_workspaces::Entity::find_by_id(
                provisioned.workspace_id,
            )
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
            let tenant =
                yorishiro::models::tenant_tenants::Model::find_by_id(&ctx.db, workspace.tenant_id)
                    .await
                    .expect("provisioned tenant");
            assert_eq!(tenant.max_workspaces, expected_tenant);
            assert_eq!(workspace.max_entities, expected_entities);
        })
        .await;
    }
}

/// The rate limit reaches the OAuth entry points by the paths the edition names when it mounts them, not by anything the limiter itself knows.
/// `status` is mounted beside them and deliberately left out, so polling it never exhausts a client's budget.
#[tokio::test]
async fn oauth_entry_points_are_rate_limited_and_status_is_not() {
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        for path in ["/auth/oauth/authorize", "/auth/oauth/callback"] {
            let mut last_status = 0;
            for _ in 0..15 {
                last_status = request.get(path).await.status_code().as_u16();
            }
            assert_eq!(
                last_status, 429,
                "{path} must share the per-IP credential budget"
            );
        }
        let mut last_status = StatusCode::OK;
        for _ in 0..15 {
            last_status = request
                .post("/auth/oauth/invites/accept")
                .json(&serde_json::json!({"invite_token":"a".repeat(48)}))
                .await
                .status_code();
        }
        assert_eq!(last_status, StatusCode::TOO_MANY_REQUESTS);
        for _ in 0..15 {
            assert_ne!(
                request.get("/auth/oauth/status").await.status_code(),
                StatusCode::TOO_MANY_REQUESTS,
                "status carries no secret and must stay unguarded"
            );
        }
    })
    .await;
}
