use crate::services::oauth::{require_non_empty, rewrite_unspecified_host};
use crate::services::{DEFAULT_BIND_ADDR, bind_addr_or_default, web_dir_or_none};

// Item 1 / item 4: `YORISHIRO_WEB_DIR`/`YORISHIRO_BIND` must treat "set to an
// empty string" the same as "unset", falling back to their defaults rather than passing an
// empty string through to `build_app`/`TcpListener::bind`.
//
// Against the constant, not a literal: what these assert is "falls back to the default", and
// spelling the port out again would make them fail on a deliberate change to it while proving
// nothing more. `the_default_port_is_the_documented_one` below is where the number itself is
// pinned, once.

#[test]
fn bind_addr_falls_back_to_default_when_unset() {
    assert_eq!(bind_addr_or_default(None), DEFAULT_BIND_ADDR);
}

#[test]
fn bind_addr_falls_back_to_default_when_set_but_empty() {
    assert_eq!(bind_addr_or_default(Some("")), DEFAULT_BIND_ADDR);
}

/// The number itself, pinned once and deliberately.
///
/// Every document, the `config.example.yml`, the Dockerfile's `EXPOSE` and the compose files
/// say 8080. The binary said 8081 for as long, so an operator who installed the package and
/// followed the documentation reached nothing -- the Docker image was the only path that
/// agreed, because it set `YORISHIRO_BIND` to paper over the difference.
///
/// Changing this constant is therefore a documentation change too. The assertion exists to make
/// that unavoidable rather than to protect the value.
#[test]
fn the_default_port_is_the_documented_one() {
    assert_eq!(DEFAULT_BIND_ADDR, "0.0.0.0:8080");
}

#[test]
fn bind_addr_passes_through_a_real_value() {
    assert_eq!(
        bind_addr_or_default(Some("127.0.0.1:9000")),
        "127.0.0.1:9000"
    );
}

#[test]
fn web_dir_is_none_when_unset() {
    assert_eq!(web_dir_or_none(None), None);
}

#[test]
fn web_dir_is_none_when_set_but_empty() {
    assert_eq!(web_dir_or_none(Some("")), None);
}

#[test]
fn web_dir_passes_through_a_real_value() {
    assert_eq!(
        web_dir_or_none(Some("/app/web")),
        Some("/app/web".to_string())
    );
}

// Item 3: `rewrite_unspecified_host` must rewrite a genuinely all-interfaces bind address to
// `localhost`, and must NOT be fooled by an address that merely contains the substring
// "0.0.0.0" -- the bug this replaced was a blind `str::replace("0.0.0.0", "localhost")`.

#[test]
fn rewrites_the_all_interfaces_ipv4_address() {
    assert_eq!(rewrite_unspecified_host("0.0.0.0:8081"), "localhost:8081");
}

#[test]
fn rewrites_the_all_interfaces_ipv6_address() {
    assert_eq!(rewrite_unspecified_host("[::]:8081"), "localhost:8081");
}

/// The regression this whole fix exists for: `10.0.0.0:8081` contains the substring
/// `"0.0.0.0"` but is a real, specific address, not an all-interfaces bind. The old
/// `bind.replace("0.0.0.0", "localhost")` turned this into `1localhost:8081` -- neither
/// `localhost` nor the original address, just corrupted. Must pass through unchanged.
#[test]
fn does_not_corrupt_an_address_that_merely_contains_the_substring() {
    assert_eq!(rewrite_unspecified_host("10.0.0.0:8081"), "10.0.0.0:8081");
}

#[test]
fn leaves_a_specific_loopback_address_unchanged() {
    assert_eq!(rewrite_unspecified_host("127.0.0.1:8081"), "127.0.0.1:8081");
}

#[test]
fn leaves_a_specific_bracketed_ipv6_address_unchanged() {
    assert_eq!(rewrite_unspecified_host("[::1]:8081"), "[::1]:8081");
}

/// A hostname isn't a valid `SocketAddr`, so it can't be parsed and rewritten -- but it's
/// already a real, browser-reachable host, so passing it through unchanged is correct, not a
/// fallback failure.
#[test]
fn leaves_an_already_valid_hostname_unchanged() {
    assert_eq!(
        rewrite_unspecified_host("example.internal:8081"),
        "example.internal:8081"
    );
}

/// An unbracketed IPv6 literal (ambiguous with `host:port` syntax) fails to parse as a
/// `SocketAddr` at all. Falling through unchanged is the safe choice -- guessing at a rewrite
/// for a malformed address would only make things worse.
#[test]
fn leaves_an_unparseable_address_unchanged() {
    assert_eq!(rewrite_unspecified_host("::1:8081"), "::1:8081");
}

// Item 2: `YORISHIRO_OAUTH_CLIENT_ID`/`YORISHIRO_OAUTH_CLIENT_SECRET` must reject an empty
// string the same as unset. The old `env::var(...).expect(...)` only caught "unset" -- `FOO=`
// satisfied it and let startup proceed with a blank client_secret, which is also the HMAC key
// for the CSRF `state` token.

#[test]
#[should_panic(expected = "YORISHIRO_OAUTH_CLIENT_SECRET must be set")]
fn require_non_empty_rejects_an_empty_string() {
    require_non_empty("YORISHIRO_OAUTH_CLIENT_SECRET", Some(""));
}

#[test]
#[should_panic(expected = "YORISHIRO_OAUTH_CLIENT_ID must be set")]
fn require_non_empty_rejects_unset() {
    require_non_empty("YORISHIRO_OAUTH_CLIENT_ID", None);
}

#[test]
fn require_non_empty_passes_through_a_real_value() {
    assert_eq!(
        require_non_empty("YORISHIRO_OAUTH_CLIENT_ID", Some("abc123")),
        "abc123"
    );
}
