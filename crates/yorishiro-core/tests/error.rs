use super::*;

/// The status/body mapping is the one every axum error wrapper delegates to, so a change here silently changes the HTTP contract of every consumer.
/// Each variant is pinned to its status.
#[test]
fn each_variant_maps_to_its_documented_status() {
    let cases = [
        (
            YorishiroError::ValidationFailed {
                message: "bad".into(),
                details: vec![],
                hint: String::new(),
            },
            422,
        ),
        (YorishiroError::not_found("gone"), 404),
        (
            YorishiroError::ScopeInsufficient {
                message: "nope".into(),
                hint: String::new(),
            },
            403,
        ),
        (
            YorishiroError::Conflict {
                message: "dupe".into(),
            },
            409,
        ),
        (
            YorishiroError::RelationTypeMismatch {
                message: "mismatch".into(),
            },
            422,
        ),
        (YorishiroError::Unauthenticated, 401),
    ];

    for (error, expected_status) in cases {
        let (status, _) = error.into_http_parts();
        assert_eq!(status, expected_status);
    }
}

/// A provider that cannot be reached is the one failure of the three an operator can act on, so it has to say which endpoint failed.
/// `502` rather than the `503` `ProviderBusy` uses: that one answered and asked for a wait, this one is a misconfiguration or an outage that waiting on the same schedule will not fix.
#[test]
fn an_unreachable_provider_names_the_endpoint() {
    let (status, body) = YorishiroError::ProviderUnreachable {
        url: "http://10.0.3.200:1234/v1".into(),
        message: "error sending request".into(),
    }
    .into_http_parts();

    assert_eq!(status, 502, "the deployment is up; its dependency is not");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("http://10.0.3.200:1234/v1"),
        "the response must name the endpoint that failed, got: {message}"
    );
    assert!(
        body["error"]["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("YORISHIRO_EMBEDDING_BASE_URL"),
        "the hint must name the setting that fixes it"
    );
}

/// The two provider failures must not collapse onto one status.
/// A caller retrying an unreachable provider on `ProviderBusy`'s schedule would be waiting out a configuration error.
#[test]
fn a_busy_provider_and_an_unreachable_one_differ() {
    let (busy, _) = YorishiroError::ProviderBusy {
        message: "rate limited".into(),
        retry_after: std::time::Duration::from_secs(30),
    }
    .into_http_parts();
    let (unreachable, _) = YorishiroError::ProviderUnreachable {
        url: "http://localhost:1".into(),
        message: "connection refused".into(),
    }
    .into_http_parts();

    assert_eq!(busy, 503);
    assert_eq!(unreachable, 502);
    assert_ne!(busy, unreachable);
}

/// An internal error must never leak its cause to the client: the detail goes to the log, and the body carries a fixed generic message.
#[test]
fn internal_errors_do_not_leak_their_cause() {
    let secret = "connection string with password";
    let (status, body) = YorishiroError::Internal(anyhow::anyhow!(secret)).into_http_parts();

    assert_eq!(status, 500);
    let rendered = body.to_string();
    assert!(!rendered.contains(secret));
    assert_eq!(body["error"]["message"], "internal server error");
}

/// `not_found` is the sanctioned constructor; it must produce the same shape as the struct literal it replaces.
#[test]
fn not_found_carries_its_message_into_the_body() {
    let (status, body) = YorishiroError::not_found("schema 'x' was not found").into_http_parts();

    assert_eq!(status, 404);
    assert_eq!(body["error"]["message"], "schema 'x' was not found");
}

/// A validation failure is the one response a caller is expected to act on, so both the per-field details and the hint have to survive into the body.
#[test]
fn validation_failures_carry_details_and_hint() {
    let error = YorishiroError::ValidationFailed {
        message: "invalid entity".into(),
        details: vec![ValidationDetail {
            field: "title".into(),
            problem: "expected string".into(),
        }],
        hint: "check the schema".into(),
    };

    let (status, body) = error.into_http_parts();

    assert_eq!(status, 422);
    assert_eq!(body["error"]["details"][0]["field"], "title");
    assert_eq!(body["error"]["details"][0]["problem"], "expected string");
    assert_eq!(body["error"]["hint"], "check the schema");
}

/// `ResultExt::internal()` exists so call sites never hand-write the `map_err`: it must map an arbitrary error into `Internal` rather than into any other variant.
#[test]
fn result_ext_maps_arbitrary_errors_to_internal() {
    let result: Result<(), std::io::Error> = Err(std::io::Error::other("disk"));

    let converted = result.internal().unwrap_err();

    assert!(matches!(converted, YorishiroError::Internal(_)));
    assert_eq!(converted.into_http_parts().0, 500);
}
