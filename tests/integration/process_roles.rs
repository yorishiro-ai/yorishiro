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
fn a_worker_process_builds_the_embedding_provider_and_fails_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let output = yorishiro(dir.path(), &["start", "--worker=query-embedding"]);
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
