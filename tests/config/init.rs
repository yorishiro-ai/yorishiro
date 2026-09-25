use std::fs;

use serial_test::serial;
use tempfile::tempdir;
use yorishiro::config::{CONFIG_SKELETON, init};

use super::CurrentDirGuard;

#[test]
#[serial(process_environment)]
fn init_refuses_existing_file_and_force_replaces_it() {
    let directory = tempdir().unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    init(false).unwrap();
    assert!(init(false).is_err());
    fs::write(
        directory.path().join("yorishiro.yaml"),
        "not the skeleton\n",
    )
    .unwrap();
    init(true).unwrap();
    assert_eq!(
        fs::read_to_string(directory.path().join("yorishiro.yaml")).unwrap(),
        CONFIG_SKELETON
    );
}

#[test]
#[serial(process_environment)]
fn init_force_creates_an_absent_target() {
    let directory = tempdir().unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    init(true).unwrap();
    assert!(directory.path().join("yorishiro.yaml").is_file());
}

#[test]
#[serial(process_environment)]
fn init_force_replaces_an_existing_regular_file_with_create_new() {
    let directory = tempdir().unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    fs::write(directory.path().join("yorishiro.yaml"), "old\n").unwrap();

    init(true).unwrap();

    assert_eq!(
        fs::read_to_string(directory.path().join("yorishiro.yaml")).unwrap(),
        CONFIG_SKELETON
    );
}

#[test]
#[serial(process_environment)]
fn init_force_rejects_a_directory() {
    let directory = tempdir().unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    fs::create_dir(directory.path().join("yorishiro.yaml")).unwrap();

    let error = init(true).unwrap_err().to_string();

    assert!(error.contains("refusing to replace directory"));
    assert!(directory.path().join("yorishiro.yaml").is_dir());
}

#[cfg(unix)]
#[test]
#[serial(process_environment)]
fn init_without_force_refuses_a_dangling_symlink() {
    let directory = tempdir().unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let path = directory.path().join("yorishiro.yaml");
    let target = directory.path().join("missing.yaml");
    std::os::unix::fs::symlink(&target, &path).unwrap();

    let error = init(false).unwrap_err().to_string();

    assert!(error.contains("refusing to overwrite"));
    assert!(path.is_symlink());
    assert!(!target.exists());
}

#[cfg(unix)]
#[test]
#[serial(process_environment)]
fn init_force_replaces_a_dangling_symlink_without_following_it() {
    let directory = tempdir().unwrap();
    let _dir = CurrentDirGuard::enter(directory.path());
    let path = directory.path().join("yorishiro.yaml");
    let target = directory.path().join("missing.yaml");
    std::os::unix::fs::symlink(&target, &path).unwrap();

    init(true).unwrap();

    assert!(!path.is_symlink());
    assert_eq!(fs::read_to_string(&path).unwrap(), CONFIG_SKELETON);
    assert!(!target.exists());
}
