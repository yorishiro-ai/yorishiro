#!/usr/bin/env -S uv run
"""Check CI responsibility ownership without a YAML dependency."""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def job(workflow: str, name: str) -> str:
    match = re.search(rf"\n  {re.escape(name)}:\n(.*?)(?=\n  [a-z][a-z0-9-]*:\n|\Z)", workflow, re.DOTALL)
    assert match, f"missing CI responsibility: {name}"
    return match[1]


def validate(workflow: str, assigned_workflow: str) -> None:
    assert "\n  test:\n" not in workflow, "legacy full-suite matrix must not return"
    assert workflow.count("scripts/test_topology.py") == 1, "only the cheap mismatch probe may use the full-suite wrapper"
    check = job(workflow, "check")
    assert "python3 scripts/test_topology.py postgres-redis models::maintenance::" in check
    assert "DATABASE_URL: sqlite:" in check and 'grep -Fxq "YORISHIRO_TOPOLOGY_REJECTED"' in check

    schema = job(workflow, "schema-generation")
    assert "${{ matrix.edition }}" in schema.splitlines()[0], "schema jobs must identify the edition"
    assert "      matrix:\n        edition: [ce, ee]\n    env:" in schema, "schema generation has only an edition axis"
    for text in (
        "needs: [changes, test-inventory]",
        "needs.changes.outputs.rust_validation == 'true'",
        "DATABASE_URL: postgres://", "QUEUE_URL: postgres://", "LOCO_ENV: test_postgres",
        "image: pgvector/pgvector:pg18", "uses: ./.github/actions/rust-setup", "components: rustfmt",
        "uses: ./.github/actions/sea-orm-cli", "name: test-executables-${{ matrix.edition }}",
        'tar -xzf "$RUNNER_TEMP/executables/test-executables-$EDITION.tar.gz" target/debug/yorishiro',
        "NOSUPERUSER NOBYPASSRLS CREATEDB CREATEROLE", "-d template1",
        "CREATE EXTENSION IF NOT EXISTS vector", "CREATE EXTENSION IF NOT EXISTS pg_trgm",
        "run: ./target/debug/yorishiro db migrate", "./target/debug/yorishiro db entities",
        "python3 scripts/harden_generated_entities.py", 'if [ "$count" -lt 10 ]',
        "git diff --exit-code -- src/models/_entities",
    ):
        assert text in schema, f"missing schema-generation guard: {text}"
    assert not re.search(r"redis|valkey|cargo (build|test|run)", schema, re.IGNORECASE), "schema generation must not run a runtime suite or rebuild the app"
    assert schema.index("-d template1") < schema.index("CREATE DATABASE yorishiro") < schema.index("run: ./target/debug/yorishiro db migrate") < schema.index("./target/debug/yorishiro db entities")

    inventory = job(workflow, "test-inventory")
    assert "edition: [ce, ee]" in inventory
    assert 'cargo test --locked --workspace --doc --no-default-features --features "$features"' in inventory
    assert 'features=community' in inventory and 'features=enterprise' in inventory
    assignment = job(workflow, "test-assignment-verify")
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
        for label in ("SCHEMA", "INVENTORY", "ASSIGNMENT", "RECEIPTS"):
            assert f'require_success {label} "${label}" {selected}' in required
    for label, flag in (("ASSIGNED", "HAS_CORE"), ("E5", "HAS_E5")):
        assert f'require_success {label} "${label}" "${flag}"' in required
        assert f'require_success {label} "${label}" false' in required


def main() -> int:
    validate((ROOT / ".github/workflows/ci.yml").read_text(), (ROOT / ".github/workflows/assigned-tests.yml").read_text())
    print("CI responsibilities: verified exact assignments, CE/EE schema generation and documentation tests")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
