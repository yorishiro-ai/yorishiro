use super::*;

/// `parse_filter_param` is `pub(crate)`: unreachable from an external integration test, and shared by the entities and search list endpoints.
/// It turns a raw query string into an optional JSON filter.
#[test]
fn an_absent_or_empty_filter_means_no_filter() {
    assert!(parse_filter_param(None).unwrap().is_none());
    assert!(parse_filter_param(Some(String::new())).unwrap().is_none());
}

/// A well-formed filter is passed through as structured JSON for the JSONB containment match.
#[test]
fn a_valid_filter_is_parsed_into_json() {
    let parsed = parse_filter_param(Some(r#"{"status":"active"}"#.into()))
        .unwrap()
        .unwrap();

    assert_eq!(parsed["status"], "active");
}

/// Malformed input comes straight from a query string, so it must produce a 422 the caller can act on (with the parser's own complaint in the hint) rather than a 500.
#[test]
fn a_malformed_filter_is_a_validation_error_carrying_a_hint() {
    let error = parse_filter_param(Some("{not json".into())).unwrap_err();

    let (status, body) = error.into_http_parts();
    assert_eq!(status, 422);
    assert!(body["error"]["hint"].as_str().unwrap().contains("filter"));
}
