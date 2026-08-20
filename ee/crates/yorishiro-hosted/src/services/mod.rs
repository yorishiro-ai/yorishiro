pub(crate) mod authz;
pub(crate) mod hmac_sign;
pub mod inference;
pub mod licence;
pub mod marketplace;
pub mod merge;
pub mod oauth;
pub mod official_templates;
pub mod origin;
pub mod plan;
pub mod tenant_auth;

// The bind address and the empty-string fold are not edition-specific, so they live in `yorishiro-server` where the community binary can reach them too.
// Re-exported rather than re-implemented: two copies of "what counts as unset" is how the two drift apart.
pub use yorishiro_server::{
    DEFAULT_BIND_ADDR, bind_addr_from_env, bind_addr_or_default, non_empty_env,
};

/// `YORISHIRO_WEB_DIR`, the directory `yorishiro-server` serves its admin dashboard SPA from.
/// `None` (unset, or set to an empty string via [`non_empty_env`]) makes the caller fall back to the community edition's own embedded assets, see `yorishiro_server::build_app`.
pub fn web_dir_from_env() -> Option<String> {
    web_dir_or_none(non_empty_env("YORISHIRO_WEB_DIR").as_deref())
}

/// The pure fold `web_dir_from_env` wraps around [`non_empty_env`]'s output, split out so it's testable without touching the process environment: `None` and `Some("")` both fold to `None`, anything else passes through unchanged.
/// Tests pass `Some("")` directly (the real call site never produces it, since `non_empty_env` already filters it out) so the empty-string case stays covered by an assertion instead of relying on `non_empty_env`'s own tests to imply it.
pub fn web_dir_or_none(raw: Option<&str>) -> Option<String> {
    raw.filter(|s| !s.is_empty()).map(str::to_string)
}

#[cfg(test)]
#[path = "../../tests/services/mod.rs"]
mod tests;
