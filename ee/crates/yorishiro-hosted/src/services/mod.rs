pub(crate) mod authz;
pub mod billing;
pub mod fill_proposals;
pub(crate) mod hmac_sign;
pub mod inference;
pub mod licence;
pub mod llm_keys;
pub mod marketplace;
pub mod merge;
pub mod oauth;
pub mod official_templates;
pub mod origin;
pub mod plan;
pub mod tenant_auth;
pub mod usage;

/// Reads an environment variable, treating both "unset" and "set to an empty string" as absent.
/// Every `_from_env()` in this crate that reads an optional variable needs this same fold --
/// `env::var(...).ok()` alone would treat `FOO=` (set but empty) as present.
pub(crate) fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

/// The default bind address when `YORISHIRO_BIND` is unset or empty.
pub const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8081";

/// `YORISHIRO_BIND`, defaulting to [`DEFAULT_BIND_ADDR`]. `FOO=` (set but empty) falls
/// back to the default the same as unset, via [`non_empty_env`], rather than being passed
/// through to `TcpListener::bind` as an empty string and aborting startup with a bind error.
pub fn bind_addr_from_env() -> String {
    bind_addr_or_default(non_empty_env("YORISHIRO_BIND").as_deref())
}

/// The pure fold `bind_addr_from_env` wraps around [`non_empty_env`]'s output, split out so it's
/// testable without touching the process environment: `None` and `Some("")` both fall back to
/// [`DEFAULT_BIND_ADDR`], anything else passes through unchanged. Tests pass `Some("")` directly
/// (the real call site never produces it, since `non_empty_env` already filters it out) so the
/// empty-string case stays covered by an assertion here rather than only implied elsewhere.
pub fn bind_addr_or_default(raw: Option<&str>) -> String {
    raw.filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_BIND_ADDR)
        .to_string()
}

/// `YORISHIRO_WEB_DIR`, the directory `yorishiro-server` serves its admin
/// dashboard SPA from. `None` (unset, or set to an empty string via [`non_empty_env`]) makes the
/// caller fall back to the community edition's own embedded assets -- see
/// `yorishiro_server::build_app`.
pub fn web_dir_from_env() -> Option<String> {
    web_dir_or_none(non_empty_env("YORISHIRO_WEB_DIR").as_deref())
}

/// The pure fold `web_dir_from_env` wraps around [`non_empty_env`]'s output, split out so it's
/// testable without touching the process environment: `None` and `Some("")` both fold to `None`,
/// anything else passes through unchanged. Tests pass `Some("")` directly (the real call site
/// never produces it, since `non_empty_env` already filters it out) so the empty-string case
/// stays covered by an assertion instead of relying on `non_empty_env`'s own tests to imply it.
pub fn web_dir_or_none(raw: Option<&str>) -> Option<String> {
    raw.filter(|s| !s.is_empty()).map(str::to_string)
}

#[cfg(test)]
#[path = "../../tests/services/mod.rs"]
mod tests;
