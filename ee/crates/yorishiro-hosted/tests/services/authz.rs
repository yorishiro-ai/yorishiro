use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use yorishiro_core::YorishiroError;

use super::*;

/// `bearer_token` is private: reachable only because this file compiles as the module's own
/// `mod tests`. It guards every dashboard request, and each rejected shape below is something a
/// real client sends: no header at all, the wrong scheme, or a header with nothing after it.
#[test]
fn only_a_non_empty_bearer_header_yields_a_token() {
    fn headers_with(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, value.parse().unwrap());
        h
    }

    assert!(matches!(
        bearer_token(&HeaderMap::new()),
        Err(YorishiroError::Unauthenticated)
    ));

    for rejected in [
        "",
        "Bearer",
        "Bearer ",
        "Basic abc123",
        "bearer ysr_lowercase",
    ] {
        assert!(
            matches!(
                bearer_token(&headers_with(rejected)),
                Err(YorishiroError::Unauthenticated)
            ),
            "expected {rejected:?} to be rejected"
        );
    }

    assert_eq!(
        bearer_token(&headers_with("Bearer ysr_abc")).unwrap(),
        "ysr_abc"
    );
}

/// The token is taken verbatim after the prefix: trimming or re-casing it here would make a
/// valid key fail to authenticate downstream.
#[test]
fn the_token_is_taken_verbatim_after_the_prefix() {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, "Bearer ysr_AbC_123 ".parse().unwrap());

    assert_eq!(bearer_token(&headers).unwrap(), "ysr_AbC_123 ");
}
