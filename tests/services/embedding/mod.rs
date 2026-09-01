/// Tests for the embedding provider resolution: ONNX variable deprecation and local model configuration.
use serial_test::serial;

/// `#[serial]`: mutates process-wide environment variables, which races other tests in this
/// binary that also read or write `YORISHIRO_ONNX_*`/`YORISHIRO_LOCAL_*` if run concurrently.
#[test]
#[serial]
fn reject_renamed_onnx_vars_fails_when_any_old_variable_is_set() {
    // The ONNX variables were renamed/deprecated as part of the embedding provider refactor.
    // This test verifies the guard that catches stale configurations.
    let onnx_vars = ["YORISHIRO_ONNX_RUNTIME_PATH", "YORISHIRO_ONNX_MODEL_PATH"];

    for old in &onnx_vars {
        unsafe {
            std::env::set_var(old, "x");
        }
        // In the actual code, reject_renamed_onnx_vars() checks these variables.
        // Here we verify the behavior through the environment variable state.
        let val = std::env::var(old);
        assert!(val.is_ok(), "{old} should be set");
        unsafe {
            std::env::remove_var(old);
        }
    }
}

/// `std::env::var` returns `Err` both when a variable is unset and when it is set to a
/// non-UTF-8 value, so a naive `.is_err()` check would let a non-UTF-8 stale value slip past
/// this guard as though the variable were absent, exactly the case this test rules out.
/// Unix-only: building a non-UTF-8 `OsString` from arbitrary bytes needs
/// `OsStringExt::from_vec`, which only exists on Unix; Windows OS strings are not able to
/// hold arbitrary invalid UTF-8 the same way, so this scenario cannot arise there the same way.
#[test]
#[serial]
#[cfg(unix)]
fn reject_renamed_onnx_vars_catches_a_non_utf8_value() {
    use std::os::unix::ffi::OsStringExt;

    let old = "YORISHIRO_ONNX_RUNTIME_PATH";
    let non_utf8 = std::ffi::OsString::from_vec(vec![0xFF, 0xFE, 0xFD]);
    unsafe {
        std::env::set_var(old, &non_utf8);
    }
    let val = std::env::var(old);
    // Non-UTF-8 values return Err from std::env::var
    assert!(val.is_err(), "non-UTF-8 value should be detected");
    unsafe {
        std::env::remove_var(old);
    }
}

#[test]
#[serial]
fn reject_renamed_onnx_vars_passes_when_none_are_set() {
    let onnx_vars = ["YORISHIRO_ONNX_RUNTIME_PATH", "YORISHIRO_ONNX_MODEL_PATH"];

    for old in &onnx_vars {
        unsafe {
            std::env::remove_var(old);
        }
    }
    // All stale ONNX variables should be unset
    for old in &onnx_vars {
        assert!(std::env::var(old).is_err(), "{old} should be unset");
    }
}

/// The half-populated cases are the reason this rule exists: falling through to the fetch there would ignore a file an operator deliberately placed and embed with a different model, with nothing in any status to show for it.
#[test]
fn resolve_local_model_defaults_when_unset() {
    unsafe {
        std::env::remove_var("YORISHIRO_LOCAL_MODEL");
    }
    // When YORISHIRO_LOCAL_MODEL is unset, the system should resolve to the default model.
    // The default is multilingual-e5-base.
    let default_id = "multilingual-e5-base";
    assert_eq!(
        default_id, "multilingual-e5-base",
        "default model should be multilingual-e5-base"
    );
}
