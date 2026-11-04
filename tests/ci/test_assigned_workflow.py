#!/usr/bin/env python3
"""Static checks for the reusable, precompiled assignment consumer."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = (ROOT / ".github/workflows/assigned-tests.yml").read_text()


class AssignedWorkflowTests(unittest.TestCase):
    def test_verified_matrix_and_matching_artifacts(self):
        for text in (
            "workflow_call:",
            "matrix: ${{ fromJSON(inputs.matrix) }}",
            "name: test-executables-${{ matrix.edition }}",
            "name: test-assignments",
            'tar -xzf "$RUNNER_TEMP/executables/test-executables-$EDITION.tar.gz" -C "$GITHUB_WORKSPACE"',
            "fail-fast: false",
        ):
            self.assertIn(text, WORKFLOW)
        self.assertNotRegex(WORKFLOW, r"cargo (build|test|run)|rust-setup|rust-cache")

    def test_topology_bootstrap_precedes_fixture_owned_migrations(self):
        self.assertIn("matrix.database == 'postgres' && 'pgvector/pgvector:pg18' || ''", WORKFLOW)
        self.assertIn("NOSUPERUSER NOBYPASSRLS CREATEDB CREATEROLE", WORKFLOW)
        self.assertIn("-d template1", WORKFLOW)
        self.assertIn("CREATE EXTENSION IF NOT EXISTS vector", WORKFLOW)
        self.assertIn("CREATE EXTENSION IF NOT EXISTS pg_trgm", WORKFLOW)
        self.assertIn('[[ "$image" == valkey/valkey:9.1.3 ]]', WORKFLOW)
        self.assertIn("if: matrix.queue == 'redis'", WORKFLOW)
        self.assertNotIn("run: ./target/debug/yorishiro db migrate", WORKFLOW)
        self.assertLess(WORKFLOW.index("CREATE DATABASE yorishiro"), WORKFLOW.index("python3 scripts/run_assigned_tests.py run"))
        for name in ("DATABASE_URL", "QUEUE_URL", "LOCO_ENV", "YORISHIRO_QUEUE_KIND"):
            self.assertIn(f"      {name}:", WORKFLOW)

    def test_embedding_coordinates_match_caller(self):
        caller = (ROOT / ".github/workflows/ci.yml").read_text()
        pattern = r"^  (EMBEDDING_\w+): (.+)$"
        self.assertEqual(dict(re.findall(pattern, caller, re.MULTILINE)), dict(re.findall(pattern, WORKFLOW, re.MULTILINE)))
        self.assertIn("uses: actions/cache/restore@v6", WORKFLOW)
        restore = WORKFLOW.split("- name: Restore multilingual-e5-base cache", 1)[1].split("- name:", 1)[0]
        self.assertIn("if: matrix.requires_e5_cache", restore)
        before = WORKFLOW.split("- name: Verify required embedding assets before execution", 1)[1].split("- name:", 1)[0]
        self.assertIn("if: matrix.requires_e5_cache", before)
        self.assertIn("run: python3 scripts/embedding_cache.py verify", WORKFLOW)
        self.assertNotIn("scripts/embedding_cache.py prepare", WORKFLOW)
        self.assertLess(WORKFLOW.index("scripts/embedding_cache.py verify"), WORKFLOW.index("python3 scripts/run_assigned_tests.py run"))
        env_source = (ROOT / "tests/env.rs").read_text()
        for name in ("YORISHIRO_TEST_E5_MODEL", "YORISHIRO_TEST_E5_TOKENIZER"):
            self.assertIn(name, env_source)
            self.assertIn(name, WORKFLOW)

    def test_assigned_execution_owns_shared_postgres_ddl_observation(self):
        bootstrap = WORKFLOW.split("- name: Bootstrap PostgreSQL extensions and non-superuser migration identity", 1)[1].split("- name: Start Valkey", 1)[0]
        self.assertIn("if: matrix.database == 'postgres'", bootstrap)
        self.assertIn("ALTER SYSTEM SET log_statement = 'ddl'", bootstrap)
        self.assertLess(WORKFLOW.index("ALTER SYSTEM SET log_statement = 'ddl'"), WORKFLOW.index("python3 scripts/run_assigned_tests.py run"))
        execution = WORKFLOW.split("- name: Execute verified exact assignments", 1)[1].split("- name:", 1)[0]
        self.assertIn("POSTGRES_CONTAINER: ${{ job.services.db.id }}", execution)
        self.assertNotIn("scripts/test_topology.py", WORKFLOW)
        caller = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertNotIn("Verify shared requests models and workers do not create databases", caller)
        self.assertNotIn("-- requests:: models:: workers::", caller)

    def test_embedding_assets_are_reverified_after_execution(self):
        verification = WORKFLOW.split("- name: Verify embedding assets after execution", 1)[1].split("- name:", 1)[0]
        self.assertIn("if: always()", verification)
        self.assertIn("matrix.requires_e5_cache", verification)
        self.assertIn("steps.execution.outcome", verification)
        self.assertIn("run: python3 scripts/embedding_cache.py verify", verification)
        self.assertLess(WORKFLOW.index("python3 scripts/run_assigned_tests.py run"), WORKFLOW.index("- name: Verify embedding assets after execution"))

    def test_reference_image_validation_does_not_block_the_model_cache(self):
        reference = (ROOT / ".github/workflows/reference-generator.yml").read_text()
        self.assertIn("tests/fixtures/generate_e5_reference/**", reference)
        self.assertIn("  schedule:", reference)
        self.assertIn("docker build -t yorishiro-e5-reference", reference)
        self.assertIn("assert not torch.version.cuda", reference)
        caller = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertNotIn("docker build -t yorishiro-e5-reference", caller)

    def test_licence_and_always_uploaded_evidence(self):
        self.assertIn("matrix.edition == 'ee' && matrix.signed_licence && secrets.YORISHIRO_LICENSE_KEY || ''", WORKFLOW)
        self.assertIn("if: matrix.signed_licence", WORKFLOW)
        self.assertIn("--output \"$RUNNER_TEMP/assigned-results/receipt.json\"", WORKFLOW)
        self.assertIn("--artifacts \"$RUNNER_TEMP/assigned-results/logs\"", WORKFLOW)
        upload = WORKFLOW.split("- name: Upload execution receipt and redacted logs", 1)[1]
        self.assertIn("if: always()", upload)
        self.assertIn("if-no-files-found: error", upload)
        self.assertIn("name: assigned-test-result-${{ matrix.edition }}-${{ matrix.job }}", upload)


if __name__ == "__main__":
    unittest.main()
