use axum::response::IntoResponse;
use yorishiro_core::YorishiroError;

use super::*;

/// `HostedApiError` exists only to bridge `YorishiroError` into axum, and the rule is that the status/body mapping stays in core's `into_http_parts()`.
/// Asserting against core's own answer rather than literals means reintroducing a local `match` here fails the test.
#[tokio::test]
async fn the_status_always_matches_what_core_maps_the_error_to() {
    let cases = [
        YorishiroError::not_found("gone"),
        YorishiroError::Unauthenticated,
        YorishiroError::Conflict {
            message: "dupe".into(),
        },
    ];

    for error in cases {
        let expected = match &error {
            YorishiroError::NotFound { message } => YorishiroError::not_found(message.clone()),
            YorishiroError::Unauthenticated => YorishiroError::Unauthenticated,
            YorishiroError::Conflict { message } => YorishiroError::Conflict {
                message: message.clone(),
            },
            _ => unreachable!(),
        }
        .into_http_parts()
        .0;

        let response = HostedApiError(error).into_response();
        assert_eq!(response.status().as_u16(), expected);
    }
}

/// An internal error must not leak its cause through this wrapper any more than through core's.
#[tokio::test]
async fn internal_errors_do_not_leak_their_cause() {
    let secret = "postgres://user:password@host/db";
    let response =
        HostedApiError(YorishiroError::Internal(anyhow::anyhow!(secret))).into_response();

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let rendered = String::from_utf8(bytes.to_vec()).unwrap();

    assert!(!rendered.contains("password"));
    assert!(rendered.contains("internal server error"));
}

/// `HostedApiErrorBody` is an OpenAPI-only type: nothing constructs it, and its doc comment says it is kept in sync with core's `into_http_parts` by hand.
/// That is exactly the kind of claim that rots silently, so the documented optional fields are checked against real bodies.
///
/// `details` and `hint` are `Option` in the documented type precisely because they are status-dependent: core emits `hint` on 403 and 422, and `details` on 422 only.
/// A client generated from this schema must treat both as absent-able.
#[tokio::test]
async fn the_documented_optional_fields_are_the_ones_core_actually_omits() {
    async fn body_of(error: YorishiroError) -> serde_json::Value {
        let response = HostedApiError(error).into_response();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    // 403: message + hint, no details.
    let forbidden = body_of(YorishiroError::ScopeInsufficient {
        message: "nope".into(),
        hint: "reissue the key".into(),
    })
    .await;
    assert!(forbidden["error"]["message"].is_string());
    assert!(forbidden["error"]["hint"].is_string());
    assert!(
        forbidden["error"].get("details").is_none(),
        "403 carries no details, so the schema must keep the field optional"
    );

    // 422: message + hint + details.
    let unprocessable = body_of(YorishiroError::ValidationFailed {
        message: "invalid".into(),
        details: vec![],
        hint: "check the schema".into(),
    })
    .await;
    assert!(unprocessable["error"]["details"].is_array());

    // 404: message only.
    let missing = body_of(YorishiroError::not_found("gone")).await;
    assert!(missing["error"]["message"].is_string());
    assert!(missing["error"].get("hint").is_none());

    // The documented type must serialise the same way: an absent field is omitted, not rendered as an explicit null.
    // Without `skip_serializing_if` the generated schema would advertise `"details": null` on every error, which no real body contains.
    let documented = serde_json::to_value(HostedApiErrorBody {
        error: HostedApiErrorDetail {
            message: "m".into(),
            details: None,
            hint: None,
        },
    })
    .unwrap();
    assert_eq!(documented["error"]["message"], "m");
    assert!(
        documented["error"].get("details").is_none(),
        "an absent `details` must be omitted, matching a real 403/404 body"
    );
    assert!(documented["error"].get("hint").is_none());
}

/// `From<YorishiroError>` is what makes `?` work in every hosted handler.
#[test]
fn a_core_error_converts_with_the_question_mark_operator() {
    fn handler() -> Result<(), HostedApiError> {
        Err(YorishiroError::Unauthenticated)?;
        Ok(())
    }
    assert!(handler().is_err());
}
