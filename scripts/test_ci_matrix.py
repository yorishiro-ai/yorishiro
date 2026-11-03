#!/usr/bin/env -S uv run
"""Deliberate violations prove the CI matrix gate rejects regressions."""

from check_ci_matrix import ROOT, validate


def main() -> int:
    workflow = (ROOT / ".github/workflows/ci.yml").read_text()
    validate(workflow)
    ddl_window = workflow.split("      - name: Verify shared requests models and workers do not create databases\n", 1)[1].split("      - name: Upload ordinary request DDL evidence\n", 1)[0]
    assert "-- requests:: models:: workers::" in ddl_window
    assert "--skip models::query_embedding_requests::expired_rows_are_purged_and_live_rows_are_kept" in ddl_window
    assert not any(line.strip().removesuffix("\\").strip() == "--skip models::query_embedding_requests::" for line in ddl_window.splitlines())
    assert "CREATE TEMP TABLE ordinary_request_ddl_probe" in ddl_window
    assert "(CREATE|DROP)[[:space:]]+DATABASE[[:space:]]" in ddl_window
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
