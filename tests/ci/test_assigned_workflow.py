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

    def test_topology_bootstrap_precedes_migration_and_execution(self):
        self.assertIn("matrix.database == 'postgres' && 'pgvector/pgvector:pg18' || ''", WORKFLOW)
        self.assertIn("NOSUPERUSER NOBYPASSRLS CREATEDB CREATEROLE", WORKFLOW)
        self.assertIn("-d template1", WORKFLOW)
        self.assertIn("CREATE EXTENSION IF NOT EXISTS vector", WORKFLOW)
        self.assertIn("CREATE EXTENSION IF NOT EXISTS pg_trgm", WORKFLOW)
        self.assertIn('[[ "$image" == valkey/valkey:9.1.3 ]]', WORKFLOW)
        self.assertIn("if: matrix.queue == 'redis'", WORKFLOW)
        self.assertEqual(WORKFLOW.count("run: ./target/debug/yorishiro db migrate"), 1)
        self.assertLess(WORKFLOW.index("CREATE DATABASE yorishiro"), WORKFLOW.index("run: ./target/debug/yorishiro db migrate"))
        self.assertLess(WORKFLOW.index("run: ./target/debug/yorishiro db migrate"), WORKFLOW.index("python3 scripts/run_assigned_tests.py run"))
        for name in ("DATABASE_URL", "QUEUE_URL", "LOCO_ENV", "YORISHIRO_QUEUE_KIND"):
            self.assertIn(f"      {name}:", WORKFLOW)

    def test_embedding_coordinates_match_caller(self):
        caller = (ROOT / ".github/workflows/ci.yml").read_text()
        pattern = r"^  (EMBEDDING_\w+): (.+)$"
        self.assertEqual(dict(re.findall(pattern, caller, re.MULTILINE)), dict(re.findall(pattern, WORKFLOW, re.MULTILINE)))
        self.assertIn("uses: actions/cache/restore@v6", WORKFLOW)
        self.assertIn("run: python3 scripts/embedding_cache.py verify", WORKFLOW)
        self.assertNotIn("scripts/embedding_cache.py prepare", WORKFLOW)
        self.assertLess(WORKFLOW.index("scripts/embedding_cache.py verify"), WORKFLOW.index("python3 scripts/run_assigned_tests.py run"))
        env_source = (ROOT / "tests/env.rs").read_text()
        for name in ("YORISHIRO_TEST_E5_MODEL", "YORISHIRO_TEST_E5_TOKENIZER"):
            self.assertIn(name, env_source)
            self.assertIn(name, WORKFLOW)

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
