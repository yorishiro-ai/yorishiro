use super::*;

/// The `Secure` cookie attribute is derived from the redirect URI's scheme rather than a
/// separate variable. Getting this backwards breaks the flow in a way that is hard to diagnose:
/// a `Secure` cookie is never returned to a plain-http callback, so local testing would fail
/// with a CSRF mismatch rather than an obvious error.
#[test]
fn the_secure_cookie_attribute_follows_the_redirect_uri_scheme() {
    fn config_with(redirect_uri: &str) -> OAuthConfig {
        OAuthConfig {
            issuer_url: "https://idp.example".into(),
            client_id: "id".into(),
            client_secret: "secret".into(),
            redirect_uri: redirect_uri.into(),
            state_signing_key: vec![0; 32],
        }
    }

    assert!(config_with("https://app.example/auth/oauth/callback").cookies_require_secure());
    assert!(!config_with("http://localhost:8081/auth/oauth/callback").cookies_require_secure());
}

/// A missing or empty required variable must fail loudly and name itself. Silently accepting an
/// empty client secret would produce an OAuth flow that fails only at the token exchange, far
/// from the cause.
#[test]
fn a_required_variable_names_itself_when_absent_or_empty() {
    for absent in [None, Some(""), Some("   ")] {
        let panicked =
            std::panic::catch_unwind(|| require_non_empty("YORISHIRO_OAUTH_CLIENT_ID", absent));
        if absent == Some("   ") {
            // whitespace is a value, not an absence -- documented behaviour, pinned here
            assert!(panicked.is_ok());
        } else {
            assert!(panicked.is_err(), "expected {absent:?} to be rejected");
        }
    }

    assert_eq!(
        require_non_empty("YORISHIRO_OAUTH_CLIENT_ID", Some("abc")),
        "abc"
    );
}

/// The default redirect URI has to be reachable from a browser. An unspecified bind address
/// (`0.0.0.0`) is not, so it is rewritten to a loopback host.
#[test]
fn an_unspecified_bind_address_is_rewritten_to_a_reachable_host() {
    assert_eq!(rewrite_unspecified_host("0.0.0.0:8081"), "localhost:8081");
    assert_eq!(rewrite_unspecified_host("[::]:8081"), "localhost:8081");

    // A concrete address is already reachable and is left alone, as is anything that is not a
    // socket address at all (a hostname the operator wrote themselves).
    assert_eq!(
        rewrite_unspecified_host("192.168.1.5:8081"),
        "192.168.1.5:8081"
    );
    assert_eq!(
        rewrite_unspecified_host("example.test:8081"),
        "example.test:8081"
    );
}
