use super::*;

#[test]
fn url_with_query_appends_params_to_a_valid_base() {
    let url = url_with_query(
        "https://idp.example.com/authorize",
        &[("a", "1"), ("b", "2")],
    )
    .unwrap();
    assert_eq!(url, "https://idp.example.com/authorize?a=1&b=2");
}

#[test]
fn url_with_query_errors_instead_of_panicking_on_a_malformed_base() {
    // A misbehaving/misconfigured provider's discovery document is untrusted, external
    // input -- this must never panic the request-handling task (see the function's doc
    // comment).
    let result = url_with_query("not a url", &[("a", "1")]);
    assert!(result.is_err());
}
