pub(crate) mod authz;
pub(crate) mod hmac_sign;
pub mod inference;
pub mod licence;
pub mod marketplace;
pub mod merge;
pub mod origin;
pub mod plan;
pub mod tenant_auth;

/// Reads an environment variable, treating both "unset" and "set to an empty string" as absent.
/// `env::var(...).ok()` alone would treat `FOO=` (set but empty) as present.
pub fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}
