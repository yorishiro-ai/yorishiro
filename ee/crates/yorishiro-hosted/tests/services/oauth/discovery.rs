use super::*;

#[test]
fn is_loopback_host_accepts_localhost_and_loopback_ips() {
    assert!(is_loopback_host("localhost"));
    assert!(is_loopback_host("127.0.0.1"));
    assert!(is_loopback_host("::1"));
}

#[test]
fn is_loopback_host_rejects_a_real_hostname() {
    assert!(!is_loopback_host("accounts.google.com"));
    assert!(!is_loopback_host("evil.example.com"));
    // A non-loopback public IP must not be mistaken for loopback either.
    assert!(!is_loopback_host("93.184.216.34"));
}

#[test]
fn require_https_or_loopback_accepts_https_and_loopback_http() {
    require_https_or_loopback("https://accounts.google.com/.well-known/openid-configuration")
        .unwrap();
    require_https_or_loopback("http://localhost:8080/.well-known/openid-configuration").unwrap();
    require_https_or_loopback("http://127.0.0.1:8080/token").unwrap();
}

#[test]
fn require_https_or_loopback_rejects_plaintext_to_a_real_host() {
    require_https_or_loopback("http://evil.example.com/.well-known/openid-configuration")
        .unwrap_err();
}

#[test]
fn is_https_or_loopback_rejects_a_public_http_url_regardless_of_where_it_was_reached_from() {
    // `redirect_policy` calls this on every redirect *target* alone: it must reject a
    // public http:// URL on its own terms, not be satisfied because some earlier hop in the
    // chain happened to be loopback or https. A local dev IdP (loopback, http://) redirecting
    // to a public http:// host is exactly the case this guards: the loopback exemption is for
    // "this request talks to my own machine", not "this request chain touched loopback once".
    let public_http = reqwest::Url::parse("http://public.example/token").unwrap();
    assert!(!is_https_or_loopback(&public_http));
}

#[test]
fn is_https_or_loopback_accepts_https_and_loopback_http_targets() {
    let https = reqwest::Url::parse("https://public.example/token").unwrap();
    let loopback_http = reqwest::Url::parse("http://127.0.0.1:8080/token").unwrap();
    assert!(is_https_or_loopback(&https));
    assert!(is_https_or_loopback(&loopback_http));
}

/// End-to-end version of the two unit tests above, through the real `reqwest::Client` built by
/// `http_client()` rather than calling `is_https_or_loopback` directly: a plain `http://`
/// loopback server (the legitimate local-dev case) issuing a `302` to a non-loopback `http://`
/// target (documented in RFC 5737 as never publicly routable, so this can never pass by
/// actually reaching a real host) must be refused, never followed.
#[tokio::test]
async fn the_real_client_refuses_to_follow_a_loopback_redirect_to_a_public_http_target() {
    use axum::response::Redirect;

    let app = axum::Router::new().route(
        "/start",
        axum::routing::get(|| async { Redirect::temporary("http://198.51.100.1/evil") }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let err = http_client()
        .get(format!("http://{addr}/start"))
        .send()
        .await
        .unwrap_err();
    assert!(
        err.is_redirect(),
        "expected the client to refuse the redirect itself (without ever attempting to \
         connect to the public target), got: {err}"
    );
}

/// `redirect_policy` accepts every hop of an all-loopback, all-`https`-or-loopback redirect
/// chain (nothing in it violates `is_https_or_loopback`), so the *only* thing that can stop an
/// attacker-controlled server from redirecting this client in a loop forever is the chain-length
/// limit `redirect_policy` delegates to `Policy::default()`. Confirms that delegation actually
/// happens: a bare `attempt.follow()` on every accepted hop (the bug `Policy::custom`'s own
/// docs warn about: a custom policy does not inherit the default 10-hop limit automatically)
/// would make this test hang or loop far past `n`.
#[tokio::test]
async fn the_real_client_gives_up_on_an_unbounded_loopback_redirect_chain() {
    let app = axum::Router::new().route(
        "/hop/{n}",
        axum::routing::get(
            |axum::extract::Path(n): axum::extract::Path<u32>| async move {
                axum::response::Redirect::temporary(&format!("/hop/{}", n + 1))
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let err = http_client()
        .get(format!("http://{addr}/hop/0"))
        .send()
        .await
        .unwrap_err();
    assert!(
        err.is_redirect(),
        "expected the client to give up on the redirect chain once it got too long \
         (inherited from Policy::default()'s 10-hop limit), got: {err}"
    );
}
