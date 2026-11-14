#!/usr/bin/env -S uv run
"""Deliberate violations prove CI responsibility guards reject regressions."""

import re

from check_ci_matrix import ROOT, validate


def main() -> int:
    workflow = (ROOT / ".github/workflows/ci.yml").read_text()
    assigned_workflow = (ROOT / ".github/workflows/assigned-tests.yml").read_text()
    validate(workflow, assigned_workflow)
    for path in (ROOT / ".github/workflows").glob("*.yml"):
        ubuntu_runners = re.findall(r"ubuntu-\d+\.\d+(?:-arm)?", path.read_text())
        assert set(ubuntu_runners) <= {"ubuntu-26.04", "ubuntu-26.04-arm"}, path
    for name in ("test-install.sh", "test-transitions.sh", "test-systemd.sh"):
        path = ROOT / ".github/packaging" / name
        images = re.findall(r"(?:ubuntu|almalinux|rockylinux):[\d.]+", path.read_text())
        assert set(images) <= {"ubuntu:26.04", "almalinux:10"}, path
    assert "FROM almalinux:10" in (ROOT / ".github/packaging/Dockerfile").read_text()
    package_action = (ROOT / ".github/actions/build-and-package/action.yml").read_text()
    assert 'RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"' in package_action
    inventory = (ROOT / ".github/workflows/test-build.yml").read_text()
    assert '--workspace --no-run --message-format=json' in inventory
    assert "scripts/test_assignments.py collect" in inventory
    assignment = workflow.split("\n  test-assignment-verify:\n", 1)[1].split("\n  package-validation:\n", 1)[0]
    assert "always()" in assignment
    assert "scripts/test_assignments.py verify" in assignment
    assert 'inventory-ce.json' in assignment and 'inventory-ee.json' in assignment
    required_check = workflow.split("\n  required-check:\n", 1)[1]
    assert 'needs.test-inventory-ce.result' in required_check
    assert 'needs.test-inventory-ee.result' in required_check
    assert 'needs.test-assignment-verify.result' in required_check
    for edition in ("CE", "EE"):
        assert f'require_success INVENTORY_{edition} "$INVENTORY_{edition}" true' in required_check
    assert 'require_success ASSIGNMENT "$ASSIGNMENT" true' in required_check
    assert 'needs.assigned-tests.result' in required_check
    assert 'needs.assigned-test-verify.result' in required_check
    for needed in ("true", "false"):
        assert f'require_success RECEIPTS "$RECEIPTS" {needed}' in required_check
    assert 'require_success ASSIGNED "$ASSIGNED" "$HAS_CORE"' in required_check
    assert 'require_success E5 "$E5" "$HAS_E5"' in required_check
    assert 'uses: ./.github/workflows/assigned-tests.yml' in workflow
    assert 'scripts/run_assigned_tests.py verify' in workflow
    assert 'name: test-executables-${{ inputs.edition }}' in inventory
    assert "Verify shared requests models and workers do not create databases" not in workflow
    assert "-- requests:: models:: workers::" not in workflow
    assert "ALTER SYSTEM SET log_statement = 'ddl'" in assigned_workflow
    assert "POSTGRES_CONTAINER: ${{ job.services.db.id }}" in assigned_workflow
    triggers = workflow.split("\non:\n", 1)[1].split("\nconcurrency:\n", 1)[0]
    assert "  pull_request:" in triggers
    assert "    branches: [develop, refactor/cli-reduction]" in triggers
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
    violations = [
        workflow + "\n  test:\n    name: Legacy full suite\n",
        workflow.replace("      edition: ee", "      edition: ce", 1),
        workflow.replace("    name: Schema generation (PostgreSQL)", "    matrix: [ce, ee]\n    name: Schema generation (PostgreSQL)", 1),
        workflow.replace("minimum == configured == actual", "True", 1),
        workflow.replace("uses: ./.github/actions/sea-orm-cli", "uses: missing-cli", 1),
        workflow.replace("components: rustfmt\n", "components: clippy\n", 1),
        workflow.replace('if [ "$count" -lt 10 ]', 'if [ "$count" -lt 0 ]', 1),
        workflow.replace("python3 scripts/harden_generated_entities.py", "echo skip-hardening", 1),
        workflow.replace("git diff --exit-code -- src/models/_entities", "echo skip-diff", 1),
        workflow.replace("NOSUPERUSER NOBYPASSRLS", "SUPERUSER BYPASSRLS", 1),
        workflow.replace("-d template1", "-d postgres", 1),
        workflow.replace("assert plan['execution_matrix_ready'], plan['cutover_blockers']", "pass", 1),
        workflow.replace("matrix: ${{ needs.test-assignment-verify.outputs.core_matrix }}", "matrix: unverified", 1),
        workflow.replace("needs: [changes, test-assignment-verify]\n", "needs: [changes, prepare-embedding, test-assignment-verify]\n", 1),
        workflow.replace("matrix: ${{ needs.test-assignment-verify.outputs.e5_matrix }}", "matrix: unverified", 1),
        workflow.replace('require_success SCHEMA "$SCHEMA" true', 'require_success SCHEMA "$SCHEMA" false', 1),
        workflow.replace("prepare-embedding, schema-generation, public-contract", "prepare-embedding, public-contract", 1),
        workflow.replace('require_success RECEIPTS "$RECEIPTS" true', 'echo skip-receipts', 1),
        workflow.replace('grep -Fxq "YORISHIRO_TOPOLOGY_REJECTED"', 'grep -Fxq "unrelated-error"', 1),
        workflow + "\n# python3 scripts/test_topology.py postgres-postgres\n",
    ]
    for invalid in violations:
        try:
            validate(invalid, assigned_workflow)
        except AssertionError:
            continue
        raise AssertionError("CI responsibility gate accepted a deliberate violation")
    for invalid in (
        inventory.replace("--workspace --doc --no-default-features", "--workspace --no-default-features", 1),
        inventory.replace("public-contract-binary-${{ inputs.edition }}", "missing-binary", 1),
        inventory.replace("      - name: Publish the small public-contract binary directly",
                          "      - if: inputs.edition == 'ce'\n        name: Publish the small public-contract binary directly", 1),
    ):
        try:
            validate(workflow, assigned_workflow, invalid)
        except AssertionError:
            continue
        raise AssertionError("CI gate accepted invalid edition artifact production")
    for invalid in (
        assigned_workflow.replace("matrix: ${{ fromJSON(inputs.matrix) }}", "matrix: unverified", 1),
        assigned_workflow.replace("POSTGRES_CONTAINER: ${{ job.services.db.id }}", "POSTGRES_CONTAINER: missing"),
        assigned_workflow.replace("ALTER SYSTEM SET log_statement = 'ddl'", "ALTER SYSTEM RESET log_statement", 1),
        assigned_workflow.replace("python3 scripts/run_assigned_tests.py run", "cargo test --workspace", 1),
    ):
        try:
            validate(workflow, invalid)
        except AssertionError:
            continue
        raise AssertionError("CI responsibility gate accepted an invalid assignment consumer")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
