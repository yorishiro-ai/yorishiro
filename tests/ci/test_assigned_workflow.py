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


class ReleaseSourceTests(unittest.TestCase):
    def test_release_packages_and_docker_use_the_prepared_commit(self):
        for name in ("release.yml", "release_pre.yml"):
            with self.subTest(workflow=name):
                text = (ROOT / ".github/workflows" / name).read_text()
                packages = text.split("\n  package-tests:\n", 1)[1].split("\n  docker:\n", 1)[0]
                self.assertIn("ref: ${{ needs.prepare.outputs.ref }}", packages)
                docker = text.split("\n  docker:\n", 1)[1].split("\n  docker-manifest:\n", 1)[0]
                self.assertIn("build-sha: ${{ needs.prepare.outputs.ref }}", docker)
        stable = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("Commit: ${{ needs.prepare.outputs.ref }}", stable)
        self.assertNotIn("Commit: ${{ github.sha }}", stable)

    def test_docker_compilation_receives_the_build_identity(self):
        action = (ROOT / ".github/actions/build-docker/action.yml").read_text()
        self.assertIn("BUILD_SHA: ${{ inputs.build-sha || inputs.tag }}", action)
        bake = (ROOT / ".github/docker/docker-bake.hcl").read_text()
        self.assertEqual(bake.count("BUILD_SHA = BUILD_SHA"), 2)
        docker = (ROOT / "Dockerfile").read_text()
        self.assertIn("ARG BUILD_SHA=dev", docker)
        self.assertLess(docker.index("ENV BUILD_SHA=${BUILD_SHA}"), docker.index("cargo build --locked"))

    def test_release_candidates_are_digest_bound_until_both_smokes_pass(self):
        for name in ("release.yml", "release_pre.yml"):
            text = (ROOT / ".github/workflows" / name).read_text()
            publisher = text.split("\n  github-release:\n", 1)[1]
            self.assertIn("needs: [binary, smoke, prepare]", publisher)
            self.assertIn("pattern: release-manifest-*", publisher)
            self.assertIn("pattern: release-smoke-*", publisher)
            self.assertIn("scripts/release_images.py promote", publisher)
            self.assertLess(publisher.index("scripts/release_images.py promote"), publisher.index("softprops/action-gh-release@"))
            self.assertEqual(text.count("source: ${{ needs.prepare.outputs.ref }}"), 2)
        build = (ROOT / ".github/actions/build-docker/action.yml").read_text()
        self.assertIn("METADATA: ${{ steps.bake.outputs.metadata }}", build)
        self.assertIn("TAG: candidate-${{ github.run_id }}", build)
        self.assertIn("scripts/release_images.py record", build)
        manifest = (ROOT / ".github/actions/docker-manifest/action.yml").read_text()
        self.assertIn("scripts/release_images.py join", manifest)
        self.assertNotIn("${TAG}-${EDITION}-amd64", manifest)
        smoke = (ROOT / ".github/actions/smoke/action.yml").read_text()
        self.assertIn('"$reference"', smoke)
        self.assertLess(smoke.index("curl -fsS http://127.0.0.1:5150/_ping ||"), smoke.index("scripts/release_images.py smoked"))


if __name__ == "__main__":
    unittest.main()
