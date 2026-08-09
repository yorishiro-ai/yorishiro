use axum::response::IntoResponse;
use yorishiro_core::YorishiroError;

use super::*;

/// `ApiError` exists only to bridge `YorishiroError` into axum, and the rule is that it
/// delegates to `into_http_parts()` rather than carrying a second `match`. If someone
/// reintroduces a local mapping, these statuses drift from core's -- so they are pinned against
/// core's own answer rather than against literals.
#[tokio::test]
async fn the_status_always_matches_what_core_maps_the_error_to() {
    let cases = [
        YorishiroError::not_found("gone"),
        YorishiroError::Unauthenticated,
        YorishiroError::Conflict {
            message: "dupe".into(),
        },
        YorishiroError::ScopeInsufficient {
            message: "nope".into(),
            hint: String::new(),
        },
    ];

    for error in cases {
        let expected_status = error.clone_status();
        let response = ApiError(error).into_response();
        assert_eq!(response.status().as_u16(), expected_status);
    }
}

/// The wrapper must not change the body either -- a consumer parsing `error.message` sees
/// whatever core produced.
#[tokio::test]
async fn the_body_is_core_s_body_verbatim() {
    let response = ApiError(YorishiroError::not_found("schema 'x' was not found")).into_response();

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(body["error"]["message"], "schema 'x' was not found");
}

/// `From<YorishiroError>` is what makes `?` work in every handler; without it each call site
/// would need an explicit map_err.
#[test]
fn a_core_error_converts_with_the_question_mark_operator() {
    fn handler() -> Result<(), ApiError> {
        Err(YorishiroError::Unauthenticated)?;
        Ok(())
    }

    assert!(handler().is_err());
}

/// Helper: ask core directly what status an error maps to, so the assertions above compare the
/// wrapper against the single source of truth instead of a copied number.
trait StatusProbe {
    fn clone_status(&self) -> u16;
}

impl StatusProbe for YorishiroError {
    fn clone_status(&self) -> u16 {
        let probe = match self {
            YorishiroError::NotFound { message } => YorishiroError::not_found(message.clone()),
            YorishiroError::Unauthenticated => YorishiroError::Unauthenticated,
            YorishiroError::Conflict { message } => YorishiroError::Conflict {
                message: message.clone(),
            },
            YorishiroError::ScopeInsufficient { message, hint } => {
                YorishiroError::ScopeInsufficient {
                    message: message.clone(),
                    hint: hint.clone(),
                }
            }
            _ => unreachable!("extend this probe if a new variant is exercised"),
        };
        probe.into_http_parts().0
    }
}
