use std::fs;

use loco_rs::environment::Environment;
use serial_test::serial;
use tempfile::tempdir;
use yorishiro::config::{CANONICAL_CONFIG_FILE, load};

use super::{CurrentDirGuard, EnvGuard, minimal_config};

#[tokio::test]
#[serial(process_environment)]
async fn canonical_file_rejects_tera_and_get_env_syntax() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join(CANONICAL_CONFIG_FILE),
        "server:\n  host: '{{ get_env(name=\"HOST\") }}'\n",
    )
    .unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&[
        "YORISHIRO_CONFIG_PATH",
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
    ]);
    _guard.remove("YORISHIRO_CONFIG_PATH");
    let error = load(&Environment::Development)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("plain YAML"));
}

#[tokio::test]
#[serial(process_environment)]
async fn only_background_queue_is_supported() {
    for mode in ["ForegroundBlocking", "BackgroundAsync"] {
        let directory = tempdir().unwrap();
        let config = format!(
            "{}\nworkers:\n  mode: {mode}\n",
            minimal_config("sqlite://file.sqlite3?mode=rwc")
        );
        fs::write(directory.path().join(CANONICAL_CONFIG_FILE), config).unwrap();
        let _dir = CurrentDirGuard::enter(directory.path());
        let _guard = EnvGuard::capture(&[
            "YORISHIRO_CONFIG_PATH",
            "DATABASE_URL",
            "QUEUE_URL",
            "YORISHIRO_QUEUE_KIND",
        ]);
        _guard.remove("YORISHIRO_CONFIG_PATH");
        _guard.remove("QUEUE_URL");
        let error = load(&Environment::Development)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("configured worker mode is not supported"));
    }
}
