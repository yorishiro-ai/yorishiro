use std::fs;

use loco_rs::environment::Environment;
use serial_test::serial;
use tempfile::tempdir;
use yorishiro::config::{CANONICAL_CONFIG_FILE, load};

use super::{CurrentDirGuard, EnvGuard, minimal_config};

#[tokio::test]
#[serial(process_environment)]
async fn explicit_path_loads_and_environment_overrides_it() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("explicit.yaml");
    fs::write(
        &path,
        minimal_config("sqlite:///from-file.sqlite3?mode=rwc"),
    )
    .unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&["YORISHIRO_CONFIG_PATH", "DATABASE_URL"]);
    _guard.set("YORISHIRO_CONFIG_PATH", &path);
    _guard.set("DATABASE_URL", "sqlite:///from-env.sqlite3?mode=rwc");
    let config = load(&Environment::Development).await.unwrap();
    assert_eq!(config.database.uri, "sqlite:///from-env.sqlite3?mode=rwc");
}

#[tokio::test]
#[serial(process_environment)]
async fn explicit_missing_path_never_falls_back() {
    let directory = tempdir().unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&["YORISHIRO_CONFIG_PATH"]);
    _guard.set(
        "YORISHIRO_CONFIG_PATH",
        directory.path().join("missing.yaml"),
    );
    let error = load(&Environment::Development)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("failed to read"));
}

#[tokio::test]
#[serial(process_environment)]
async fn explicit_unreadable_path_never_falls_back() {
    let directory = tempdir().unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&["YORISHIRO_CONFIG_PATH"]);
    _guard.set("YORISHIRO_CONFIG_PATH", directory.path());
    let error = load(&Environment::Development)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("failed to read"));
}

#[tokio::test]
#[serial(process_environment)]
async fn explicit_invalid_path_never_falls_back() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("explicit.yaml");
    fs::write(&path, "logger: [not valid for Config\n").unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&["YORISHIRO_CONFIG_PATH"]);
    _guard.set("YORISHIRO_CONFIG_PATH", path);
    let error = load(&Environment::Development)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("explicit.yaml"));
}

#[tokio::test]
#[serial(process_environment)]
async fn canonical_file_in_current_directory_wins() {
    let directory = tempdir().unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&[
        "YORISHIRO_CONFIG_PATH",
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
    ]);
    fs::write(
        directory.path().join(CANONICAL_CONFIG_FILE),
        minimal_config("sqlite:///canonical.sqlite3?mode=rwc"),
    )
    .unwrap();
    for variable in [
        "YORISHIRO_CONFIG_PATH",
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
    ] {
        _guard.remove(variable);
    }
    let config = load(&Environment::Development).await.unwrap();
    assert_eq!(config.database.uri, "sqlite:///canonical.sqlite3?mode=rwc");
}

#[tokio::test]
#[serial(process_environment)]
async fn canonical_file_wins_when_legacy_file_also_exists() {
    let directory = tempdir().unwrap();
    let legacy_directory = directory.path().join("config");
    fs::create_dir(&legacy_directory).unwrap();
    fs::write(
        legacy_directory.join("development.yaml"),
        minimal_config("sqlite:///legacy.sqlite3?mode=rwc"),
    )
    .unwrap();
    fs::write(
        directory.path().join(CANONICAL_CONFIG_FILE),
        minimal_config("sqlite:///canonical.sqlite3?mode=rwc"),
    )
    .unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&[
        "YORISHIRO_CONFIG_PATH",
        "LOCO_CONFIG_FOLDER",
        "DATABASE_URL",
        "QUEUE_URL",
        "YORISHIRO_QUEUE_KIND",
    ]);
    _guard.remove("YORISHIRO_CONFIG_PATH");
    _guard.set("LOCO_CONFIG_FOLDER", &legacy_directory);
    _guard.remove("DATABASE_URL");
    _guard.remove("QUEUE_URL");
    _guard.remove("YORISHIRO_QUEUE_KIND");
    let config = load(&Environment::Development).await.unwrap();
    assert_eq!(config.database.uri, "sqlite:///canonical.sqlite3?mode=rwc");
}

#[tokio::test]
#[serial(process_environment)]
async fn legacy_environment_file_is_the_final_fallback() {
    let directory = tempdir().unwrap();
    let config_directory = directory.path().join("config");
    fs::create_dir(&config_directory).unwrap();
    fs::write(
        config_directory.join("test_postgres.yaml"),
        minimal_config("{{ get_env(name=\"DATABASE_URL\") }}"),
    )
    .unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let _guard = EnvGuard::capture(&[
        "YORISHIRO_CONFIG_PATH",
        "LOCO_CONFIG_FOLDER",
        "DATABASE_URL",
    ]);
    _guard.remove("YORISHIRO_CONFIG_PATH");
    _guard.set("LOCO_CONFIG_FOLDER", &config_directory);
    _guard.set("DATABASE_URL", "postgres://test:test@localhost:5432/test");
    let config = load(&Environment::Any("test_postgres".into()))
        .await
        .unwrap();
    assert_eq!(
        config.database.uri,
        "postgres://test:test@localhost:5432/test"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial(process_environment)]
async fn dangling_canonical_symlink_does_not_fall_back_to_legacy_config() {
    let directory = tempdir().unwrap();
    std::os::unix::fs::symlink(
        directory.path().join("missing.yaml"),
        directory.path().join(CANONICAL_CONFIG_FILE),
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
    _guard.remove("DATABASE_URL");
    _guard.remove("QUEUE_URL");
    _guard.remove("YORISHIRO_QUEUE_KIND");
    let error = load(&Environment::Development)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("failed to read"));
}
