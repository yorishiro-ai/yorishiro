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
    let _guard = EnvGuard::new(vec!["YORISHIRO_CONFIG_PATH", "YORISHIRO_BIND"]);
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "bind: 127.0.0.1:9000\n");
    // SAFETY: serialized by ENV_LOCK via EnvGuard.
    unsafe { std::env::set_var("YORISHIRO_CONFIG_PATH", &path) };

    unsafe { load_and_apply_env_overrides() }.unwrap();

    assert_eq!(std::env::var("YORISHIRO_BIND").unwrap(), "127.0.0.1:9000");
}

#[test]
fn env_var_wins_over_yaml_value() {
    let _guard = EnvGuard::new(vec!["YORISHIRO_CONFIG_PATH", "YORISHIRO_BIND"]);
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "bind: 127.0.0.1:9000\n");
    // SAFETY: serialized by ENV_LOCK via EnvGuard.
    unsafe {
        std::env::set_var("YORISHIRO_CONFIG_PATH", &path);
        std::env::set_var("YORISHIRO_BIND", "127.0.0.1:1234");
    }

    unsafe { load_and_apply_env_overrides() }.unwrap();

    assert_eq!(std::env::var("YORISHIRO_BIND").unwrap(), "127.0.0.1:1234");
}

#[test]
fn missing_config_file_is_a_no_op() {
    let _guard = EnvGuard::new(vec!["YORISHIRO_CONFIG_PATH", "YORISHIRO_BIND"]);
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_LOCK via EnvGuard.
    unsafe {
        std::env::set_var(
            "YORISHIRO_CONFIG_PATH",
            dir.path().join("does-not-exist.yml"),
        )
    };

    unsafe { load_and_apply_env_overrides() }.unwrap();

    assert!(std::env::var_os("YORISHIRO_BIND").is_none());
}

#[test]
fn nested_embedding_settings_are_applied() {
    let _guard = EnvGuard::new(vec![
        "YORISHIRO_CONFIG_PATH",
        "YORISHIRO_EMBEDDING_PROVIDER",
        "YORISHIRO_EMBEDDING_DIMENSIONS",
        "YORISHIRO_ONNX_MODEL_PATH",
    ]);
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        "embedding:\n  provider: local\n  dimensions: 768\n  onnx_model_path: /models/model.onnx\n",
    );
    // SAFETY: serialized by ENV_LOCK via EnvGuard.
    unsafe { std::env::set_var("YORISHIRO_CONFIG_PATH", &path) };

    unsafe { load_and_apply_env_overrides() }.unwrap();

    assert_eq!(
        std::env::var("YORISHIRO_EMBEDDING_PROVIDER").unwrap(),
        "local"
    );
    assert_eq!(
        std::env::var("YORISHIRO_EMBEDDING_DIMENSIONS").unwrap(),
        "768"
    );
    assert_eq!(
        std::env::var("YORISHIRO_ONNX_MODEL_PATH").unwrap(),
        "/models/model.onnx"
    );
}

#[test]
fn unknown_key_is_a_hard_error() {
    let _guard = EnvGuard::new(vec!["YORISHIRO_CONFIG_PATH"]);
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "not_a_real_setting: true\n");
    // SAFETY: serialized by ENV_LOCK via EnvGuard.
    unsafe { std::env::set_var("YORISHIRO_CONFIG_PATH", &path) };

    let err = unsafe { load_and_apply_env_overrides() }.unwrap_err();

    assert!(err.to_string().contains("failed to parse config file"));
}

/// `config.example.yml` offers both of these, and `FileConfig` is `deny_unknown_fields` — so a
/// key documented there but missing from the struct does not silently do nothing, it refuses to
/// start. Covering them here keeps the example file and the loader from drifting apart.
#[test]
fn search_and_snapshot_settings_are_applied_and_overridable() {
    let _guard = EnvGuard::new(vec![
        "YORISHIRO_CONFIG_PATH",
        "YORISHIRO_SEARCH_TOKENS_PER_MINUTE",
        "YORISHIRO_SNAPSHOT_RETENTION_DAYS",
    ]);
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        "search_tokens_per_minute: 5000\nsnapshot_retention_days: 7\n",
    );
    // SAFETY: serialized by ENV_LOCK via EnvGuard.
    unsafe {
        std::env::set_var("YORISHIRO_CONFIG_PATH", &path);
        // Set one of the two, so this asserts both directions in one run.
        std::env::set_var("YORISHIRO_SNAPSHOT_RETENTION_DAYS", "90");
    }

    unsafe { load_and_apply_env_overrides() }.unwrap();

    assert_eq!(
        std::env::var("YORISHIRO_SEARCH_TOKENS_PER_MINUTE").unwrap(),
        "5000",
        "the yaml value is applied when the environment says nothing"
    );
    assert_eq!(
        std::env::var("YORISHIRO_SNAPSHOT_RETENTION_DAYS").unwrap(),
        "90",
        "the environment still wins over the file"
    );
}

/// `config.example.yml` is `deny_unknown_fields`-parsed like any other config, so a key
/// documented there but absent from `FileConfig` makes a copied example refuse to start. This
/// parses the example with every key uncommented, which is the only way that mismatch shows up
/// before a user hits it.
#[test]
fn the_example_config_parses_with_every_key_enabled() {
    let example = include_str!("../../../../config.example.yml");
    let enabled: String = example
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix('#')?;
            let indent = rest.len() - rest.trim_start().len();
            let body = rest.trim_start();
            // Only "key: value" lines; prose comments are skipped.
            let key = body.split(':').next()?;
            if key.is_empty()
                || !key.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                || !body.contains(':')
            {
                return None;
            }
            let body = body.split("  #").next()?.trim_end();
            Some(format!("{}{}", " ".repeat(indent.saturating_sub(1)), body))
        })
        .collect::<Vec<_>>()
        .join("\n");

    let parsed = serde_yaml_ng::from_str::<super::FileConfig>(&enabled);
    assert!(
        parsed.is_ok(),
        "config.example.yml does not parse into FileConfig: {:?}\n--- what was parsed ---\n{enabled}",
        parsed.err()
    );
}

/// `license_key` belongs to the paid edition, and both editions parse this struct, which is
/// `deny_unknown_fields`. So the field has to exist here or a config carrying the key refuses
/// to start on the community build -- an operator switching editions would meet that.
///
/// The field is otherwise unused, which makes it exactly the kind of thing a later cleanup
/// removes. This is the contract that says it cannot be.
#[test]
fn a_licence_key_in_the_config_is_accepted() {
    let _guard = EnvGuard::new(vec!["YORISHIRO_CONFIG_PATH", "YORISHIRO_BIND"]);
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        "license_key: a-token\nbind: 127.0.0.1:9000\n",
    );
    // SAFETY: serialized by ENV_LOCK via EnvGuard.
    unsafe { std::env::set_var("YORISHIRO_CONFIG_PATH", &path) };

    unsafe { load_and_apply_env_overrides() }.unwrap();

    // Parsed, so the settings beside it took effect.
    assert_eq!(std::env::var("YORISHIRO_BIND").unwrap(), "127.0.0.1:9000");
}

/// And it must not reach the environment from here.
///
/// This loader is compiled into the community binary. Applying the key would put the string
/// `YORISHIRO_LICENSE_KEY` into that artifact, which the release gate scans for and rejects --
/// the build is meant to carry no trace of the paid edition. `ee/` reads the file itself.
#[test]
fn a_licence_key_in_the_config_is_not_applied_to_the_environment() {
    let _guard = EnvGuard::new(vec!["YORISHIRO_CONFIG_PATH", "YORISHIRO_LICENSE_KEY"]);
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "license_key: a-token\n");
    // SAFETY: serialized by ENV_LOCK via EnvGuard.
    unsafe { std::env::set_var("YORISHIRO_CONFIG_PATH", &path) };

    unsafe { load_and_apply_env_overrides() }.unwrap();

    assert!(std::env::var("YORISHIRO_LICENSE_KEY").is_err());
}
