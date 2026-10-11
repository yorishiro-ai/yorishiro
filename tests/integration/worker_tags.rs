//! Integration test that invokes the actual yorishiro binary's `worker-tags`
//! subcommand and asserts its exact stdout format.

use std::process::Command;

use yorishiro::edition::worker_tags;
use yorishiro::workers::embedding_sync::WorkerClass;

/// Invokes the compiled binary and asserts `worker-tags` output format.
#[test]
#[serial_test::serial(process_environment)]
fn worker_tags_command_output_is_single_comma_separated_line() {
    let bin = env!("CARGO_BIN_EXE_yorishiro");
    let output = Command::new(bin)
        .args(["worker-tags"])
        .output()
        .expect("run yorishiro");
    assert!(output.status.success(), "yorishiro worker-tags must exit 0");
    let stdout = String::from_utf8(output.stdout).expect("valid UTF-8");

    // Must be exactly one line (tags joined by commas, terminated with newline).
    let lines: Vec<&str> = stdout.trim_end().split('\n').collect();
    assert_eq!(
        lines.len(),
        1,
        "worker-tags must print exactly one line; got {} lines",
        lines.len()
    );

    let tags_csv = lines[0];
    let expected = worker_tags().join(",");
    assert_eq!(
        tags_csv, expected,
        "worker-tags output must match worker_tags()"
    );
}

/// Every expected tag must appear exactly once in the `worker-tags` output.
#[test]
#[serial_test::serial(process_environment)]
fn worker_tags_command_includes_all_expected_tags() {
    let bin = env!("CARGO_BIN_EXE_yorishiro");
    let output = Command::new(bin)
        .args(["worker-tags"])
        .output()
        .expect("run yorishiro");
    assert!(output.status.success(), "yorishiro worker-tags must exit 0");
    let stdout = String::from_utf8(output.stdout).expect("valid UTF-8");

    let tags_csv = stdout.trim_end();
    let received: Vec<&str> = tags_csv.split(',').collect();
    let expected_tags = worker_tags();

    // Must match count exactly.
    assert_eq!(
        received.len(),
        expected_tags.len(),
        "worker-tags returned {} tags but expected {}",
        received.len(),
        expected_tags.len()
    );

    // Every expected tag must be present exactly once.
    for expected in &expected_tags {
        let count = received.iter().filter(|t| **t == expected.as_str()).count();
        assert_eq!(
            count, 1,
            "tag {:?} must appear exactly once; found {} occurrences",
            expected, count
        );
    }
}

/// Base worker-class tags must all be present.
#[test]
#[serial_test::serial(process_environment)]
fn worker_tags_command_contains_base_worker_class_tags() {
    let bin = env!("CARGO_BIN_EXE_yorishiro");
    let output = Command::new(bin)
        .args(["worker-tags"])
        .output()
        .expect("run yorishiro");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("valid UTF-8");
    let tags_csv = stdout.trim_end();

    for class in WorkerClass::ALL {
        let tag = class.tag();
        assert!(
            tags_csv.contains(tag),
            "worker-tags output must include base tag {tag} for {class:?}"
        );
    }
}

/// Enterprise `infer-fill` tag must be present when compiled in.
#[cfg(feature = "enterprise")]
#[test]
#[serial_test::serial(process_environment)]
fn worker_tags_command_contains_infer_fill() {
    let bin = env!("CARGO_BIN_EXE_yorishiro");
    let output = Command::new(bin)
        .args(["worker-tags"])
        .output()
        .expect("run yorishiro");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("valid UTF-8");
    let tags_csv = stdout.trim_end();
    assert!(
        tags_csv.contains("infer-fill"),
        "worker-tags must include the enterprise infer-fill tag"
    );
}

/// The binary prints exactly the baseline in a community build and the baseline plus the edition's worker in an enterprise one, independent of the library function the other tests compare against.
#[test]
#[serial_test::serial(process_environment)]
fn worker_tags_command_prints_the_edition_exact_list() {
    let bin = env!("CARGO_BIN_EXE_yorishiro");
    let output = Command::new(bin)
        .args(["worker-tags"])
        .output()
        .expect("run yorishiro");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("valid UTF-8");
    let baseline =
        "worker-class:tenant-private,worker-class:official,worker-class:shared,query-embedding";
    #[cfg(feature = "enterprise")]
    let expected = format!("{baseline},infer-fill");
    #[cfg(not(feature = "enterprise"))]
    let expected = baseline.to_owned();
    assert_eq!(stdout.trim_end(), expected);
}
