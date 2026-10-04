pub mod oauth;
pub mod plan;

/// Reads an environment variable, treating both unset and empty as absent.
pub fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}
