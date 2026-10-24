#!/usr/bin/env -S uv run
"""Deliberate violations prove the CI matrix gate rejects regressions."""

from check_ci_matrix import ROOT, validate


def main() -> int:
    workflow = (ROOT / ".github/workflows/ci.yml").read_text()
    validate(workflow)
    triggers = workflow.split("\non:\n", 1)[1].split("\nconcurrency:\n", 1)[0]
    assert "  pull_request:" in triggers
    assert "    branches: [develop]" in triggers
    assert not any(
        line.strip().split(":", 1)[0] in {"push", "workflow_dispatch", "schedule"}
        for line in triggers.splitlines()
        if ":" in line
    ), "CI must run only for pull requests"
    cleanup = (ROOT / ".github/workflows/cache-cleanup.yml").read_text()
    cleanup_triggers = cleanup.split("\non:\n", 1)[1].split("\npermissions:\n", 1)[0]
    assert "  pull_request_target:" in cleanup_triggers
    assert "    types: [closed]" in cleanup_triggers
    assert "    branches: [develop]" in cleanup_triggers
    job = cleanup.split("\njobs:\n", 1)[1]
    assert "actions: write" in cleanup
    assert "actions/checkout" not in cleanup
    assert "github.event.pull_request.head" not in cleanup
    assert "refs/pull/$PR_NUMBER/merge" in job
    assert '[[ ! "$PR_NUMBER" =~ ^[0-9]+$ ]]' in job
    assert "gh cache delete --all --ref" in job
    assert 'env:\n          GH_TOKEN: ${{ github.token }}' in job
    assert 'REPO: ${{ github.repository }}' in job
    assert 'PR_NUMBER: ${{ github.event.pull_request.number }}' in job
    docker_build = (ROOT / ".github/actions/build-docker/action.yml").read_text()
    manifest = (ROOT / ".github/actions/docker-manifest/action.yml").read_text()
    description = "An MCP-native, multi-tenant knowledge store with user-defined schemas."
    bake = (ROOT / "docker-bake.hcl").read_text()
    assert f"manifest:org.opencontainers.image.description={description}" in bake
    assert "docker/bake-action@" in docker_build
    assert "RUST_VERSION" not in docker_build
    dockerfile = (ROOT / "Dockerfile").read_text()
    assert 'COPY rust-toolchain.toml .' in dockerfile
    assert '"toolchain"]["channel"]' in dockerfile
    assert "cargo install sccache" not in dockerfile
    assert f"DESCRIPTION: {description}" in manifest
    assert '"index:org.opencontainers.image.description=${DESCRIPTION}"' in manifest
    assert 'docker buildx imagetools create \\\n          --annotation' in manifest
    matrix_start = workflow.index("          - edition: ce", workflow.index("\n  test:\n"))
    first_end = workflow.index("          - edition: ce", matrix_start + 1)
    first = workflow[matrix_start:first_end]
    violations = [
        workflow[:matrix_start] + workflow[first_end:],  # Missing pair.
        workflow[:matrix_start] + first + workflow[matrix_start:],  # Duplicate pair.
        workflow.replace("queue_key: postgres", "queue_key: sqlite", 1),  # Crossed SQL pair.
        workflow.replace("        include:", "        edition: [ce, ee]\n        include:", 1),  # Original axis/include bug.
        workflow.replace("${{ matrix.edition }}", "hidden-edition"),
    ]
    for invalid in violations:
        try:
            validate(invalid)
        except (AssertionError, IndexError):
            continue
        raise AssertionError("CI matrix gate accepted a deliberate violation")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
