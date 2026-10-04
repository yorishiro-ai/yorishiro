use yorishiro::ee::data::oauth::*;

#[test]
fn require_non_empty_accepts_a_present_value() {
    assert_eq!(require_non_empty("KEY", Some("value")).unwrap(), "value");
}

#[test]
fn require_non_empty_rejects_an_unset_value() {
    let err = require_non_empty("KEY", None).unwrap_err();
    assert!(
        err.to_string()
            .contains("KEY must be set to a non-empty value")
    );
}

#[test]
fn require_non_empty_rejects_a_set_but_empty_value() {
    let err = require_non_empty("KEY", Some("")).unwrap_err();
    assert!(
        err.to_string()
            .contains("KEY must be set to a non-empty value")
    );
}

#[test]
fn rewrite_unspecified_host_rewrites_all_interfaces_addresses() {
    assert_eq!(rewrite_unspecified_host("0.0.0.0:8080"), "localhost:8080");
    assert_eq!(rewrite_unspecified_host("[::]:8080"), "localhost:8080");
}

#[test]
fn rewrite_unspecified_host_leaves_a_real_address_alone() {
    // Must not be corrupted by a substring replace: this merely contains "0.0.0.0".
    assert_eq!(rewrite_unspecified_host("10.0.0.0:8081"), "10.0.0.0:8081");
    assert_eq!(rewrite_unspecified_host("127.0.0.1:8080"), "127.0.0.1:8080");
}

#[test]
fn rewrite_unspecified_host_leaves_a_non_socket_addr_alone() {
    assert_eq!(
        rewrite_unspecified_host("example.com:8080"),
        "example.com:8080"
    );
}
