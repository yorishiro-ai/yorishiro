use std::sync::Mutex;

use crate::config::load_and_apply_env_overrides;

// Env vars are process-wide state; serialize tests through this lock rather than racing
// each other (same pattern as `yorishiro_core::repositories::tenancy`'s env tests).
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    keys: Vec<&'static str>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn new(keys: Vec<&'static str>) -> Self {
        // A test that panics while holding this poisons the mutex; a plain `unwrap()` would
        // turn one real failure into a cascade of unrelated ones. `Drop` restores the vars
        // either way, so the state behind the poisoned lock is not suspect.
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for key in &keys {
            // SAFETY: serialized by ENV_LOCK, no other threads touch these keys.
            unsafe { std::env::remove_var(key) };
        }
        Self { keys, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for key in &self.keys {
            // SAFETY: serialized by ENV_LOCK, no other threads touch these keys.
            unsafe { std::env::remove_var(key) };
        }
    }
}

fn write_config(dir: &std::path::Path, yaml: &str) -> std::path::PathBuf {
    let path = dir.join("config.yml");
    std::fs::write(&path, yaml).unwrap();
    path
}

#[test]
fn yaml_value_is_applied_when_env_is_unset() {
    let _guard = EnvGuard::new(vec!["YSR_CONFIG_PATH", "YSR_BIND"]);
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "bind: 127.0.0.1:9000\n");
    // SAFETY: serialized by ENV_LOCK via EnvGuard.
    unsafe { std::env::set_var("YSR_CONFIG_PATH", &path) };

    unsafe { load_and_apply_env_overrides() }.unwrap();

    assert_eq!(std::env::var("YSR_BIND").unwrap(), "127.0.0.1:9000");
}

#[test]
fn env_var_wins_over_yaml_value() {
    let _guard = EnvGuard::new(vec!["YSR_CONFIG_PATH", "YSR_BIND"]);
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "bind: 127.0.0.1:9000\n");
    // SAFETY: serialized by ENV_LOCK via EnvGuard.
    unsafe {
        std::env::set_var("YSR_CONFIG_PATH", &path);
        std::env::set_var("YSR_BIND", "127.0.0.1:1234");
    }

    unsafe { load_and_apply_env_overrides() }.unwrap();

    assert_eq!(std::env::var("YSR_BIND").unwrap(), "127.0.0.1:1234");
}

#[test]
fn missing_config_file_is_a_no_op() {
    let _guard = EnvGuard::new(vec!["YSR_CONFIG_PATH", "YSR_BIND"]);
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_LOCK via EnvGuard.
    unsafe { std::env::set_var("YSR_CONFIG_PATH", dir.path().join("does-not-exist.yml")) };

    unsafe { load_and_apply_env_overrides() }.unwrap();

    assert!(std::env::var_os("YSR_BIND").is_none());
}

#[test]
fn nested_embedding_settings_are_applied() {
    let _guard = EnvGuard::new(vec![
        "YSR_CONFIG_PATH",
        "YSR_EMBEDDING_PROVIDER",
        "YSR_EMBEDDING_DIMENSIONS",
        "YSR_ONNX_MODEL_PATH",
    ]);
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        "embedding:\n  provider: local\n  dimensions: 768\n  onnx_model_path: /models/model.onnx\n",
    );
    // SAFETY: serialized by ENV_LOCK via EnvGuard.
    unsafe { std::env::set_var("YSR_CONFIG_PATH", &path) };

    unsafe { load_and_apply_env_overrides() }.unwrap();

    assert_eq!(std::env::var("YSR_EMBEDDING_PROVIDER").unwrap(), "local");
    assert_eq!(std::env::var("YSR_EMBEDDING_DIMENSIONS").unwrap(), "768");
    assert_eq!(
        std::env::var("YSR_ONNX_MODEL_PATH").unwrap(),
        "/models/model.onnx"
    );
}

#[test]
fn unknown_key_is_a_hard_error() {
    let _guard = EnvGuard::new(vec!["YSR_CONFIG_PATH"]);
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "not_a_real_setting: true\n");
    // SAFETY: serialized by ENV_LOCK via EnvGuard.
    unsafe { std::env::set_var("YSR_CONFIG_PATH", &path) };

    let err = unsafe { load_and_apply_env_overrides() }.unwrap_err();

    assert!(err.to_string().contains("failed to parse config file"));
}
