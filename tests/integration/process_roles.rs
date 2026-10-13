//! The real binary, run in the roles that must not load an embedding model.
//!
//! The environment names a local model that does not exist, so any command that built the provider would fail with "not a known local model".

use std::process::{Command, Output};

fn yorishiro(dir: &std::path::Path, args: &[&str]) -> Output {
    let uri = |name: &str| format!("sqlite://{}?mode=rwc", dir.join(name).display());
    Command::new(env!("CARGO_BIN_EXE_yorishiro"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("LOCO_ENV", "test_sqlite")
        .env("DATABASE_URL", uri("app.sqlite3"))
        .env("QUEUE_URL", uri("queue.sqlite3"))
        .env("YORISHIRO_EMBEDDING_PROVIDER", "local")
        .env(
            "YORISHIRO_LOCAL_MODEL",
            "nonexistent-model-that-does-not-exist",
        )
        .output()
        .expect("run yorishiro")
}

/// A CLI command runs in a process of its own and holds no model.
#[test]
#[serial_test::serial(process_environment)]
fn a_cli_command_runs_without_building_the_embedding_provider() {
    let dir = tempfile::tempdir().unwrap();
    let output = yorishiro(dir.path(), &["routes"]);
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The same environment stops a worker at start, which shows the environment really would have failed the command above if it had built a provider.
#[test]
#[serial_test::serial(process_environment)]
fn a_worker_process_builds_the_embedding_provider_and_fails_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let output = yorishiro(dir.path(), &["start", "--worker=worker-target:shared"]);
    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("not a known local model"),
        "the worker names the unknown model: {combined}"
    );
}

fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// An accepted worker start reaches provider construction, which fails on this test's unknown model.
fn assert_accepted(dir: &std::path::Path, args: &[&str]) {
    let output = yorishiro(dir, args);
    assert!(!output.status.success(), "{args:?}");
    assert!(
        combined(&output).contains("not a known local model"),
        "{args:?} was refused before the provider was built: {}",
        combined(&output)
    );
}

/// A start that is refused says why and builds nothing: the unknown model never gets to fail it.
fn assert_refused(dir: &std::path::Path, args: &[&str], reason: &str) {
    let output = yorishiro(dir, args);
    assert!(!output.status.success(), "{args:?} must be refused");
    let message = combined(&output);
    assert!(message.contains(reason), "{args:?}: {message}");
    assert!(
        message.contains("worker-target:shared"),
        "{args:?} names the target to use: {message}"
    );
    assert!(
        !message.contains("not a known local model"),
        "{args:?} was refused before the provider was built: {message}"
    );
}

/// Every spelling loco-rs's command line accepts for `--worker` names the same targets.
#[test]
#[serial_test::serial(process_environment)]
fn a_worker_target_is_read_from_every_form_the_command_line_accepts() {
    let dir = tempfile::tempdir().unwrap();
    let shared = "worker-target:shared";
    for args in [
        vec!["start", "--worker=worker-target:shared"],
        vec!["start", "--worker", shared],
        vec!["start", "-w", shared],
        vec!["start", "-w=worker-target:shared"],
        vec!["s", "--worker=worker-target:shared"],
        vec![
            "start",
            "--worker=worker-target:shared,worker-target:official",
        ],
        vec!["start", "--worker", shared, "worker-target:official"],
        vec![
            "start",
            "--worker=worker-target:shared",
            "--worker=worker-target:official",
        ],
        vec!["start", "--worker=worker-target:shared", "--scheduler"],
        vec![
            "start",
            "--worker=worker-target:shared",
            "--no-banner",
            "--port",
            "5151",
        ],
        vec![
            "-e",
            "test_sqlite",
            "start",
            "--worker=worker-target:shared",
        ],
        vec![
            "start",
            "--worker=worker-target:tenant:0198f3a0-0000-7000-8000-00000000000a",
        ],
    ] {
        assert_accepted(dir.path(), &args);
    }
}

/// A bare `--worker` names no target, and a process that serves nothing would sit idle, so it is refused rather than given a default.
#[test]
#[serial_test::serial(process_environment)]
fn a_worker_started_without_a_target_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["start", "--worker"],
        vec!["start", "-w"],
        vec!["s", "--worker", "--scheduler"],
    ] {
        assert_refused(dir.path(), &args, "names no target");
    }
}

/// A tag that names no target is refused before anything is built, rather than starting a worker that waits for jobs nobody sends it.
#[test]
#[serial_test::serial(process_environment)]
fn a_worker_started_with_an_invalid_target_is_refused_before_it_builds_anything() {
    let dir = tempfile::tempdir().unwrap();
    for tag in [
        "worker-class:shared",
        "query-embedding",
        "infer-fill",
        "worker-target:other",
        "worker-target:tenant:not-a-uuid",
        "worker-target:tenant:0198F3A0-0000-7000-8000-00000000000A",
    ] {
        assert_refused(
            dir.path(),
            &["start", &format!("--worker={tag}")],
            "invalid worker target",
        );
    }
    // One bad tag beside a good one still refuses the whole start.
    assert_refused(
        dir.path(),
        &[
            "start",
            "--worker=worker-target:shared,worker-class:official",
        ],
        "invalid worker target",
    );
}

/// Loco gives these modes no tags, and every job Yorishiro enqueues is tagged, so their workers would sit idle.
#[test]
#[serial_test::serial(process_environment)]
fn a_start_that_runs_workers_without_a_target_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["start", "--all"],
        vec!["start", "--server-and-worker"],
        vec!["start", "--server-and-worker", "--scheduler"],
        vec!["start", "-a"],
        vec!["start", "-s"],
    ] {
        assert_refused(dir.path(), &args, "cannot be given a worker target");
    }
}

/// Commands that start no worker are not read for targets, so an unrelated command line never fails on them.
#[test]
#[serial_test::serial(process_environment)]
fn commands_that_start_no_worker_are_not_affected() {
    let dir = tempfile::tempdir().unwrap();
    for args in [vec!["routes"], vec!["version"]] {
        let output = yorishiro(dir.path(), &args);
        assert!(output.status.success(), "{args:?}: {}", combined(&output));
    }
}
