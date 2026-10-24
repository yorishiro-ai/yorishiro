#!/usr/bin/env python3
"""Focused contract tests for change-based CI selection."""

from classify_ci_changes import ALL, classify, classify_path, package_build_allowed, selected_job_succeeded


def test_unknown_defaults_to_full() -> None:
    assert classify_path("new-unmapped-file.bin") == ALL
    assert classify_path(".github/new-policy.yml") == ALL
    assert classify_path(".github/workflows/unknown.yml") == ALL


def test_docs_are_lightweight() -> None:
    assert classify_path("ee/docs/installation.md") == {"docs"}
    assert classify(["ee/docs/installation.md"])["docs_only"]
    assert classify_path("ee/models/example.rs") == {"ee"}
    result = classify(["docs/en/contributing.md", "docs/ja/contributing.md"])
    assert result["docs_only"]
    assert not result["rust_validation"]
    assert not result["package_validation"]


def test_dependency_patches_always_select_rust_and_security() -> None:
    for path in ("patches/loco-rs/fixes.patch", "patches/loco-rs/source/src/boot.rs", "patches/serde_yaml/src/lib.rs", "patches/README.md"):
        result = classify([path])
        assert result["rust_validation"] and result["security_validation"]
        assert result["rust"] and result["ee"]
        assert not result["docs_only"]


def test_packaging_selects_package_and_rust() -> None:
    result = classify([".github/packaging/nfpm-yorishiro-ce.yaml"])
    assert result["package_build"] and result["package_validation"]
    assert result["rust"] and result["ee"]
    assert not result["full"]
    assert classify_path("src/lib.rs") == {"shared", "rust", "ee"}


def test_release_workflow_selects_all_validations() -> None:
    result = classify([".github/workflows/release_pre.yml"])
    assert result["release"]
    assert result["package_validation"]
    assert result["docker_validation"]
    assert classify([".github/docker/docker-bake.hcl"])["docker_validation"]


def test_manual_full_run_selects_every_group() -> None:
    result = classify([], force_full=True)
    assert result["full"]
    assert all(result[group] for group in ALL)


def test_housekeeping_workflow_is_light_but_unknown_workflow_is_full() -> None:
    housekeeping = classify([".github/workflows/stale.yml"])
    assert not housekeeping["package_build"]
    assert not housekeeping["rust_validation"]
    unknown = classify([".github/workflows/new-job.yml"])
    assert unknown["full"]
    assert unknown["package_build"] and unknown["rust_validation"]


def test_shared_actions_fan_out_to_consumers() -> None:
    rust = classify([".github/actions/rust-setup/action.yml"])
    assert rust["rust"] and rust["ee"] and rust["deps"] and rust["packaging"] and rust["release"]
    package = classify([".github/actions/package-tests/action.yml"])
    assert package["packaging"] and package["docker"] and package["release"]


def test_missing_paths_force_full_and_manual_force_is_full() -> None:
    assert classify([], force_full=True)["full"]
    assert classify(["docs/ja/contributing.md"])["docs_only"]


def test_selected_gate_states_fail_unless_success_or_intentionally_skipped() -> None:
    assert selected_job_succeeded("success", True)
    assert not selected_job_succeeded("failure", True)
    assert not selected_job_succeeded("cancelled", True)
    assert not selected_job_succeeded("skipped", True)
    assert selected_job_succeeded("skipped", False)
    assert not selected_job_succeeded("success", False)


def test_package_and_docker_groups_are_independent() -> None:
    docker = classify(["Dockerfile"])
    assert docker["package_build"] and docker["package_validation"]
    assert docker["docker_validation"]
    docs = classify(["docs/en/contributing.md"])
    assert not docs["package_build"] and not docs["package_validation"]
    dependency = classify(["Cargo.lock"])
    assert dependency["security_validation"]


def test_package_build_prerequisite_selection() -> None:
    assert package_build_allowed(True, False, "success", "skipped", "skipped", "skipped")
    assert not package_build_allowed(True, True, "success", "success", "failure", "success")
    assert not package_build_allowed(True, False, "cancelled", "skipped", "skipped", "skipped")
    assert not package_build_allowed(False, False, "success", "skipped", "skipped", "skipped")


if __name__ == "__main__":
    for test in (
        test_unknown_defaults_to_full,
        test_docs_are_lightweight,
        test_dependency_patches_always_select_rust_and_security,
        test_packaging_selects_package_and_rust,
        test_release_workflow_selects_all_validations,
        test_manual_full_run_selects_every_group,
        test_housekeeping_workflow_is_light_but_unknown_workflow_is_full,
        test_shared_actions_fan_out_to_consumers,
        test_missing_paths_force_full_and_manual_force_is_full,
        test_selected_gate_states_fail_unless_success_or_intentionally_skipped,
        test_package_and_docker_groups_are_independent,
        test_package_build_prerequisite_selection,
    ):
        test()
    print("change classifier: all assertions passed")
