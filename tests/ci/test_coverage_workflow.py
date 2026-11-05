#!/usr/bin/env python3
"""Coverage follows compiled ownership instead of rerunning the full suite on PostgreSQL."""

import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("coverage_report", ROOT / "scripts/coverage_report.py")
coverage = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(coverage)


class CoverageOwnershipTests(unittest.TestCase):
    def test_topology_clears_inherited_provider_and_licence_settings(self):
        base = {"YORISHIRO_LICENSE_KEY": "signed", "LLVM_PROFILE_FILE": "profiles/%m-%p.profraw",
                "YORISHIRO_POSTGRES_TEST_URL": "old-pg", "YORISHIRO_REDIS_TEST_URL": "old-redis",
                "YORISHIRO_TEST_REDIS_BASE_URL": "old-redis", "YORISHIRO_TEST_E5_MODEL": "old-model",
                "YORISHIRO_TEST_E5_TOKENIZER": "old-tokenizer"}
        cell = {"edition": "ce", "job": "common-sqlite", "database": "sqlite", "queue": "sqlite"}
        env = coverage.assignment_environment(cell, base, Path("reports"))
        self.assertEqual(env["LOCO_ENV"], "test_sqlite")
        self.assertNotEqual(env["DATABASE_URL"], env["QUEUE_URL"])
        self.assertEqual(env["YORISHIRO_LICENSE_KEY"], "")
        self.assertNotIn("YORISHIRO_POSTGRES_TEST_URL", env)
        self.assertNotIn("YORISHIRO_REDIS_TEST_URL", env)
        self.assertNotIn("YORISHIRO_TEST_E5_MODEL", env)
        self.assertEqual(env["LLVM_PROFILE_FILE"], base["LLVM_PROFILE_FILE"])
        cell.update(edition="ee", database="postgres", queue="redis", signed_licence=True)
        env = coverage.assignment_environment(cell, base, Path("reports"))
        self.assertEqual(env["LOCO_ENV"], "test_postgres_redis")
        self.assertEqual(env["YORISHIRO_LICENSE_KEY"], "signed")
        self.assertEqual(env["YORISHIRO_QUEUE_KIND"], "Redis")
        with self.assertRaisesRegex(RuntimeError, "signed EE licence"):
            coverage.assignment_environment(cell, {}, Path("reports"))
        cell.update(database="sqlite", queue="sqlite", signed_licence=False, requires_e5_cache=True)
        env = coverage.assignment_environment(cell, base, Path("reports"))
        self.assertTrue(env["YORISHIRO_TEST_E5_MODEL"].endswith("model.safetensors"))
        self.assertTrue(env["YORISHIRO_TEST_E5_TOKENIZER"].endswith("tokenizer.json"))

    def test_exact_plan_runs_each_cell_once_with_its_edition_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "coverage"
            output.mkdir()
            main = root / "target/debug/yorishiro"
            test = root / "target/debug/deps/mod-example"
            main.parent.mkdir(parents=True)
            test.parent.mkdir()
            cells = [{"edition": "ce", "job": "common-sqlite", "database": "sqlite", "queue": "sqlite"},
                     {"edition": "ee", "job": "licensed-sqlite", "database": "sqlite", "queue": "sqlite",
                      "signed_licence": True},
                     {"edition": "ee", "job": "postgres-redis", "database": "postgres", "queue": "redis",
                      "signed_licence": True}]
            executed = []

            def fake_run(command, **kwargs):
                if "build" in command:
                    edition = "ee" if "enterprise" in command else "ce"
                    main.write_text(edition)
                    test.write_text(edition)
                elif "scripts/test_assignments.py" in command:
                    destination = Path(command[command.index("--output") + 1])
                    if "collect" in command:
                        data = {"artifacts": [{"executable": str(test)}],
                                "process_binary": {"executable": str(main)}}
                    else:
                        data = {"execution_matrix_ready": True, "execution_matrix": {"include": cells}}
                    destination.write_text(json.dumps(data))
                elif "scripts/run_assigned_tests.py" in command and "run" in command:
                    edition = command[command.index("--edition") + 1]
                    self.assertEqual(main.read_text(), edition)
                    self.assertEqual(test.read_text(), edition)
                    executed.append((edition, command[command.index("--job") + 1],
                                     kwargs["env"]["YORISHIRO_LICENSE_KEY"]))

            env = {"GITHUB_ACTIONS": "true", "LLVM_PROFILE_FILE": "profiles/%m-%p.profraw",
                   "CARGO_TARGET_DIR": str(root / "target"), "YORISHIRO_LICENSE_KEY": "signed",
                   "POSTGRES_CONTAINER": "coverage-db"}
            with mock.patch.object(coverage, "ROOT", root), mock.patch.dict(os.environ, env), \
                    mock.patch.object(coverage.subprocess, "run", side_effect=fake_run) as process:
                coverage.run_assignments(output, "+nightly")
            self.assertEqual(executed, [("ce", "common-sqlite", ""), ("ee", "licensed-sqlite", "signed"),
                                        ("ee", "postgres-redis", "signed")])
            commands = [call.args[0] for call in process.call_args_list]
            docker = [command for command in commands if command[0] == "docker"]
            self.assertEqual(len(docker), 2)
            self.assertIn("DROP DATABASE yorishiro WITH (FORCE)", docker[0])
            self.assertEqual(docker[1], ["docker", "exec", "coverage-redis", "valkey-cli", "FLUSHALL"])
            self.assertEqual(sum("tests/blackbox/run.py" in command for command in commands), 3)
            self.assertEqual(sum("verify" in command and "scripts/run_assigned_tests.py" in command
                                 for command in commands), 1)
            self.assertTrue(all("--no-run" in command for command in commands if "test" in command))
            self.assertFalse(any(path.is_file() for path in (output / "binaries").rglob("*")))

    def test_workflow_owns_real_prerequisites_and_keeps_branch_module_reports(self):
        workflow = (ROOT / ".github/workflows/coverage.yml").read_text()
        self.assertNotIn("sea-orm-cli", workflow)
        self.assertNotIn("make coverage", workflow)
        self.assertIn("secrets.YORISHIRO_LICENSE_KEY", workflow)
        self.assertIn("NOSUPERUSER NOBYPASSRLS", workflow)
        self.assertIn("POSTGRES_CONTAINER: ${{ job.services.db.id }}", workflow)
        self.assertIn("show-env --sh --branch", workflow)
        self.assertIn("RUSTUP_TOOLCHAIN: nightly", workflow)
        self.assertIn("if: always()", workflow)
        source = (ROOT / "scripts/coverage_report.py").read_text()
        for artifact in ("modules.json", "modules.csv", "lowest-covered.txt"):
            self.assertIn(artifact, source)
        self.assertIn('metric(summary, "branches")', source)
        self.assertIn('"report",', source)
        report = source.split('    command = [', 1)[1].split('    subprocess.run(command', 1)[0]
        for option in ('--no-default-features', '--features', '--workspace', '--branch'):
            self.assertNotIn(option, report)


if __name__ == "__main__":
    unittest.main()
