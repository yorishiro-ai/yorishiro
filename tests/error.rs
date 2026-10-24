use std::error::Error;

use yorishiro::error::YorishiroError;

#[test]
fn internal_error_keeps_its_message_conversion_and_source_chain() {
    let error = YorishiroError::from(anyhow::anyhow!("root cause").context("operation failed"));
    assert_eq!(error.to_string(), "internal error: operation failed");
    let source = error.source().expect("internal source");
    assert_eq!(source.to_string(), "operation failed");
    assert_eq!(
        source.source().expect("root cause").to_string(),
        "root cause"
    );
}

#[test]
fn domain_errors_keep_their_display_and_have_no_source() {
    let errors = [
        (YorishiroError::Unauthenticated, "unauthenticated"),
        (
            YorishiroError::Conflict {
                message: "duplicate".into(),
            },
            "conflict: duplicate",
        ),
        (
            YorishiroError::ProviderUnreachable {
                url: "http://localhost".into(),
                message: "offline".into(),
            },
            "embedding provider unreachable at http://localhost: offline",
        ),
    ];
    for (error, message) in errors {
        assert_eq!(error.to_string(), message);
        assert!(error.source().is_none());
    }
}

#[test]
fn rest_and_loco_boundaries_do_not_expose_internal_details() {
    let (status, body) =
        YorishiroError::Internal(anyhow::anyhow!("private diagnostic")).into_http_parts();
    assert_eq!(status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"]["message"], "internal server error");
    assert!(!body.to_string().contains("private diagnostic"));

    let error = loco_rs::Error::from(YorishiroError::Internal(anyhow::anyhow!(
        "private diagnostic"
    )));
    let loco_rs::Error::CustomError(loco_status, detail) = error else {
        panic!("application error must use Loco's custom error boundary");
    };
    assert_eq!(loco_status, status);
    assert_eq!(detail.errors, Some(body));
}
