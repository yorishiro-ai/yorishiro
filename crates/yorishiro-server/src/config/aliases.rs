//! The `YSR_` → `YORISHIRO_` rename, and the transition period that keeps old names working.
//!
//! Two prefixes existed because two binaries did. One binary is left, so one prefix is, and
//! `YORISHIRO_` is the one that survives: it names the product, and every variable that was
//! already documented on the paid side (OAuth, Stripe, the licence key) uses it.
//!
//! An old name still works and logs a warning naming its replacement. Silently breaking a
//! deployment that has been setting `YSR_BIND` for months is not an acceptable way to make a
//! naming scheme tidy.

/// Old name → new name, for every renamed variable.
///
/// `YORISHIRO_HOSTED_*` loses the infix as well: "hosted" distinguished a binary that no longer
/// has a counterpart. `YSR_WEB_DIR` and `YORISHIRO_HOSTED_WEB_DIR` both map onto the one
/// `YORISHIRO_WEB_DIR`, which is what ends the double-documented pair.
pub const RENAMES: &[(&str, &str)] = &[
    ("YSR_BIND", "YORISHIRO_BIND"),
    ("YORISHIRO_HOSTED_BIND", "YORISHIRO_BIND"),
    ("YSR_WEB_DIR", "YORISHIRO_WEB_DIR"),
    ("YORISHIRO_HOSTED_WEB_DIR", "YORISHIRO_WEB_DIR"),
    ("YSR_CONFIG_PATH", "YORISHIRO_CONFIG_PATH"),
    ("YSR_CORS_ORIGINS", "YORISHIRO_CORS_ORIGINS"),
    ("YSR_AUTH_RATE_LIMIT_MAX", "YORISHIRO_AUTH_RATE_LIMIT_MAX"),
    (
        "YSR_AUTH_RATE_LIMIT_WINDOW_SECS",
        "YORISHIRO_AUTH_RATE_LIMIT_WINDOW_SECS",
    ),
    ("YSR_DB_LOAD_POLL_SECS", "YORISHIRO_DB_LOAD_POLL_SECS"),
    ("YSR_DB_LOAD_SUSTAIN_SECS", "YORISHIRO_DB_LOAD_SUSTAIN_SECS"),
    ("YSR_DB_LOAD_THRESHOLD", "YORISHIRO_DB_LOAD_THRESHOLD"),
    ("YSR_EMBEDDING_API_KEY", "YORISHIRO_EMBEDDING_API_KEY"),
    ("YSR_EMBEDDING_BASE_URL", "YORISHIRO_EMBEDDING_BASE_URL"),
    ("YSR_EMBEDDING_DIMENSIONS", "YORISHIRO_EMBEDDING_DIMENSIONS"),
    ("YSR_EMBEDDING_MODEL", "YORISHIRO_EMBEDDING_MODEL"),
    ("YSR_EMBEDDING_PROVIDER", "YORISHIRO_EMBEDDING_PROVIDER"),
    (
        "YSR_EMBEDDING_SEND_DIMENSIONS_PARAM",
        "YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM",
    ),
    ("YSR_LOG_DIR", "YORISHIRO_LOG_DIR"),
    ("YSR_LOG_TARGET", "YORISHIRO_LOG_TARGET"),
    (
        "YSR_ONNX_MAX_SEQUENCE_LENGTH",
        "YORISHIRO_ONNX_MAX_SEQUENCE_LENGTH",
    ),
    ("YSR_ONNX_MODEL_PATH", "YORISHIRO_ONNX_MODEL_PATH"),
    ("YSR_ONNX_POOLING", "YORISHIRO_ONNX_POOLING"),
    (
        "YSR_ONNX_QUERY_INSTRUCTION",
        "YORISHIRO_ONNX_QUERY_INSTRUCTION",
    ),
    ("YSR_ONNX_TOKENIZER_PATH", "YORISHIRO_ONNX_TOKENIZER_PATH"),
    (
        "YSR_SEARCH_TOKENS_PER_MINUTE",
        "YORISHIRO_SEARCH_TOKENS_PER_MINUTE",
    ),
    (
        "YSR_SNAPSHOT_RETENTION_DAYS",
        "YORISHIRO_SNAPSHOT_RETENTION_DAYS",
    ),
    ("YSR_SYSLOG_SOCKET", "YORISHIRO_SYSLOG_SOCKET"),
];

/// What [`apply`] would do, as a pure function over the variables it was handed.
///
/// Returns the `(new_name, value)` pairs to set and the `(old, new)` pairs to warn about. Split
/// out so the mapping is testable without touching the process environment -- which is
/// process-wide state that would make these tests race every other test in the binary.
///
/// A new name that is already set wins: an operator who has migrated one variable and not
/// another is mid-transition, not misconfigured, and the name they moved to is the one they
/// mean.
pub fn plan<'a>(
    lookup: impl Fn(&str) -> Option<String>,
    renames: &'a [(&'a str, &'a str)],
) -> Vec<(&'a str, &'a str, String)> {
    renames
        .iter()
        .filter_map(|(old, new)| {
            let value = lookup(old)?;
            if lookup(new).is_some() {
                return None;
            }
            Some((*old, *new, value))
        })
        .collect()
}

/// Copies any old-name variable onto its new name and warns for each.
///
/// Must run *before* `load_and_apply_env_overrides`: that function only sets a variable when it
/// is unset, so a config file would otherwise win over an explicitly exported old name, quietly
/// inverting the precedence a deployment already relies on.
///
/// # Safety
///
/// Calls `std::env::set_var`, which is unsound with concurrent environment access. Call from
/// `main`'s synchronous prologue, before the tokio runtime starts.
pub unsafe fn apply() {
    for (old, new, value) in plan(|k| std::env::var(k).ok(), RENAMES) {
        // Not `tracing::warn!`: this runs before the subscriber is installed, so a tracing event
        // here would be swallowed. `eprintln!` is what actually reaches the operator.
        eprintln!("warning: {old} is deprecated, use {new} (the old name still works for now)");
        // SAFETY: forwarded from this function's own contract.
        unsafe { std::env::set_var(new, value) };
    }
}

#[cfg(test)]
#[path = "../../tests/config/aliases.rs"]
mod tests;
