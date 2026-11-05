#!/usr/bin/env -S uv run
"""Check CI responsibility ownership without a YAML dependency."""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def job(workflow: str, name: str) -> str:
    match = re.search(rf"\n  {re.escape(name)}:\n(.*?)(?=\n  [a-z][a-z0-9-]*:\n|\Z)", workflow, re.DOTALL)
    assert match, f"missing CI responsibility: {name}"
    return match[1]


def validate(workflow: str, assigned_workflow: str, build_workflow: str | None = None) -> None:
    if build_workflow is None:
        build_workflow = (ROOT / ".github/workflows/test-build.yml").read_text()
    assert "\n  test:\n" not in workflow, "legacy full-suite matrix must not return"
    assert workflow.count("scripts/test_topology.py") == 1, "only the cheap mismatch probe may use the full-suite wrapper"
    check = job(workflow, "check")
    assert "python3 scripts/test_topology.py postgres-redis models::maintenance::" in check
    assert "DATABASE_URL: sqlite:" in check and 'grep -Fxq "YORISHIRO_TOPOLOGY_REJECTED"' in check
    assert "minimum == configured == actual" in check, "validation must compile on the declared minimum compiler"
    assert "\n  msrv:\n" not in workflow and "make check-ee" not in check
    assert "make check-ce" not in job(workflow, "ce")
    assert check.count("cargo clippy") == 1 and "--force-warn clippy::cognitive_complexity" in check

    schema = job(workflow, "schema-generation")
    assert "matrix:" not in schema, "edition-neutral migrations need one PostgreSQL regeneration"
    for text in (
        "needs: [changes, test-inventory-ce]",
        "needs.changes.outputs.rust_validation == 'true'",
        "DATABASE_URL: postgres://", "QUEUE_URL: postgres://", "LOCO_ENV: test_postgres",
        "image: pgvector/pgvector:pg18", "uses: ./.github/actions/rust-setup", "components: rustfmt",
        "uses: ./.github/actions/sea-orm-cli", "name: test-executables-ce",
        'tar -xzf "$RUNNER_TEMP/executables/test-executables-ce.tar.gz" target/debug/yorishiro',
        "NOSUPERUSER NOBYPASSRLS CREATEDB CREATEROLE", "-d template1",
        "CREATE EXTENSION IF NOT EXISTS vector", "CREATE EXTENSION IF NOT EXISTS pg_trgm",
        "run: ./target/debug/yorishiro db migrate", "./target/debug/yorishiro db entities",
        "python3 scripts/harden_generated_entities.py", 'if [ "$count" -lt 10 ]',
        "git diff --exit-code -- src/models/_entities",
    ):
        assert text in schema, f"missing schema-generation guard: {text}"
    assert not re.search(r"redis|valkey|cargo (build|test|run)", schema, re.IGNORECASE), "schema generation must not run a runtime suite or rebuild the app"
    assert schema.index("-d template1") < schema.index("CREATE DATABASE yorishiro") < schema.index("run: ./target/debug/yorishiro db migrate") < schema.index("./target/debug/yorishiro db entities")

    for edition in ("ce", "ee"):
        inventory = job(workflow, f"test-inventory-{edition}")
        assert "uses: ./.github/workflows/test-build.yml" in inventory
        assert f"edition: {edition}" in inventory
    assert 'cargo test --locked --workspace --doc --no-default-features --features "$features"' in build_workflow
    assert "features=community" in build_workflow and "features=enterprise" in build_workflow
    assert build_workflow.index("cargo build") < build_workflow.index("scripts/test_assignments.py collect")
    assert "public-contract-binary-${{ inputs.edition }}" in build_workflow
    assert 'public-contract-$EDITION.tar.gz' in build_workflow
    assert "if: inputs.edition == 'ce'" not in build_workflow, "both editions publish their own binary"
    assert "process_binary" in build_workflow
    assert "\n  public-contract-build:\n" not in workflow
    for consumer in ("public-contract", "schema-generation"):
        assert "needs: [changes, test-inventory-ce]" in job(workflow, consumer), "CE consumers must not wait for EE"
    assert "needs: [changes, test-inventory-ee]" in job(workflow, "public-contract-ee")
    assert "public-contract-ee" in job(workflow, "public-contract-verify")
    assignment = job(workflow, "test-assignment-verify")
    assert "needs: [changes, test-inventory-ce, test-inventory-ee]" in assignment
    assert "scripts/test_assignments.py verify" in assignment
    assert "assert plan['execution_matrix_ready'], plan['cutover_blockers']" in assignment
    assigned = job(workflow, "assigned-tests")
    assert "uses: ./.github/workflows/assigned-tests.yml" in assigned
    assert "matrix: ${{ needs.test-assignment-verify.outputs.core_matrix }}" in assigned
    assert "prepare-embedding" not in assigned, "ordinary tests must not wait for model artifacts"
    e5 = job(workflow, "assigned-e5-tests")
    assert "needs: [changes, prepare-embedding, test-assignment-verify]" in e5
    assert "matrix: ${{ needs.test-assignment-verify.outputs.e5_matrix }}" in e5
    assert "outputs.has_core == 'true'" in assigned and "outputs.has_e5 == 'true'" in e5
    assert "if: matrix.requires_e5_cache" in assigned_workflow
    assert "always() && matrix.requires_e5_cache" in assigned_workflow
    models = job(workflow, "prepare-embedding")
    assert "docker build" not in models and "setup-uv" not in models
    assert "python3 scripts/embedding_cache.py prepare" in models
    assert "matrix: ${{ fromJSON(inputs.matrix) }}" in assigned_workflow
    assert "POSTGRES_CONTAINER: ${{ job.services.db.id }}" in assigned_workflow
    assert "ALTER SYSTEM SET log_statement = 'ddl'" in assigned_workflow, "assigned execution owns PostgreSQL DDL evidence"
    assert "python3 scripts/run_assigned_tests.py run" in assigned_workflow
    assert not re.search(r"cargo (build|test|run)|scripts/test_topology.py", assigned_workflow)
    required = job(workflow, "required-check")
    dependencies = re.search(r"^    needs: \[([^\n]+)\]$", required, re.MULTILINE)
    assert dependencies and "schema-generation" in dependencies[1].split(", ")
    assert "test" not in dependencies[1].split(", ")
    assert "needs.schema-generation.result" in required
    assert "needs.test.result" not in required and "require_success TEST " not in required
    for selected in ("true", "false"):
        for label in ("SCHEMA", "INVENTORY_CE", "INVENTORY_EE", "ASSIGNMENT", "RECEIPTS"):
            assert f'require_success {label} "${label}" {selected}' in required
    for label, flag in (("ASSIGNED", "HAS_CORE"), ("E5", "HAS_E5")):
        assert f'require_success {label} "${label}" "${flag}"' in required
        assert f'require_success {label} "${label}" false' in required


def main() -> int:
    validate((ROOT / ".github/workflows/ci.yml").read_text(), (ROOT / ".github/workflows/assigned-tests.yml").read_text())
    print("CI responsibilities: verified edition builds, shared schema, exact execution and resource dependencies")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
