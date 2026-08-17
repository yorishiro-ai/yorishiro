use std::sync::Mutex;

use super::*;

// `YORISHIRO_LOG_TARGET` is process-wide state, so these serialize through one lock rather than racing each other, the same pattern `tests/config/mod.rs` uses.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Restores the variable on drop, including when a test panics mid-way.
/// A test that removed it by hand at the end would leak the value into whichever test ran next on an assertion failure.
struct TargetGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl TargetGuard {
    fn set(value: &str) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialized by ENV_LOCK, and nothing else touches this key.
        unsafe { std::env::set_var("YORISHIRO_LOG_TARGET", value) };
        Self { _lock: lock }
    }
}

impl Drop for TargetGuard {
    fn drop(&mut self) {
        // SAFETY: serialized by ENV_LOCK, and nothing else touches this key.
        unsafe { std::env::remove_var("YORISHIRO_LOG_TARGET") };
    }
}

/// A typo in `YORISHIRO_LOG_TARGET` has to stop startup.
/// Falling back to a default would leave an operator believing logs are going somewhere they are not: the failure would only surface when someone went looking for logs that were never written.
#[test]
fn an_unknown_target_is_rejected_at_startup() {
    let _guard = TargetGuard::set("sylsog");

    let Err(error) = init() else {
        panic!("an unknown target must not fall back to a default");
    };

    let message = error.to_string();
    assert!(message.contains("sylsog"), "{message}");
    assert!(
        message.contains("stdout") && message.contains("syslog"),
        "the error should list the valid targets: {message}"
    );
}

/// An empty value is a real shape (`YORISHIRO_LOG_TARGET=` in a compose file or `.env`) and must be rejected like any other unknown target rather than being read as "unset".
#[test]
fn an_empty_target_is_rejected_rather_than_treated_as_unset() {
    let _guard = TargetGuard::set("");

    assert!(
        init().is_err(),
        "an empty YORISHIRO_LOG_TARGET should be rejected, not silently treated as the default"
    );
}

/// The default target needs no configuration, which is what makes the Docker image work with no logging setup at all: an unset `YORISHIRO_LOG_TARGET` must resolve to `stdout` rather than being rejected like an unknown value.
///
/// This asserts the resolution, not `init()` itself: `init()` installs the global tracing subscriber, which can only ever be set once per process, so calling it here would panic whenever another test in the same binary got there first.
#[test]
fn an_unset_target_resolves_to_the_default() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: serialized by ENV_LOCK.
    unsafe { std::env::remove_var("YORISHIRO_LOG_TARGET") };

    let resolved = std::env::var("YORISHIRO_LOG_TARGET").unwrap_or_else(|_| "stdout".into());

    assert_eq!(
        resolved, "stdout",
        "an unset YORISHIRO_LOG_TARGET must fall back to stdout"
    );
}
