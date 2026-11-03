#!/usr/bin/env python3
"""Synthetic scheduler checks: no Rust binaries, databases, providers, or real child processes."""

import copy
import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("assigned_execution", Path(__file__).resolve().parents[2] / "scripts/run_assigned_tests.py")
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class AssignedExecutionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.binary = self.root / "target/debug/deps/mod-synthetic"
        self.binary.parent.mkdir(parents=True)
        self.binary.write_text("not an executable test program")
        self.binary.chmod(0o700)
        self.manifest = {"jobs": {"common-sqlite": {"database": "sqlite", "queue": "sqlite", "signed_licence": False}}}
        self.plan = {"schema": 1, "inventory_valid": True, "revision": "synthetic-revision",
                     "jobs": self.manifest["jobs"], "pending_splits": [], "helpers": [], "assignments": [], "artifacts": []}
        for edition in ("ce", "ee"):
            self.plan["artifacts"].append({"edition": edition, "package": "yorishiro", "target": "mod", "kind": "test",
                                           "executable": "/old/build/target/debug/deps/mod-synthetic",
                                           "sha256": runner.inventory.file_digest(self.binary), "tests": ["a", "a::nested", "b", "helper"],
                                           "ignored": [], "features": ["community"] + (["enterprise"] if edition == "ee" else [])})
        for name, edition in (("a", "ce"), ("b", "ce"), ("a", "ee")):
            self.plan["assignments"].append({"package": "yorishiro", "target": "mod", "kind": "test", "name": name,
                                             "owner": edition, "role": "ee_delta" if edition == "ee" else "common",
                                             "jobs": ["common-sqlite"]})
        self.env = {"DATABASE_URL": "sqlite:///application.sqlite?mode=rwc", "QUEUE_URL": "sqlite:///queue.sqlite?mode=rwc",
                    "LOCO_ENV": "test_sqlite"}
        self.calls = []

    def fake_process(self, argv, env, deadline, log):
        self.calls.append((argv, env, deadline))
        self.assertEqual(env["YORISHIRO_TEST_STRICT_TOPOLOGY"], "true")
        self.assertGreater(deadline, runner.time.monotonic())
        if argv[0] == "docker":
            text = runner.DDL_PROBE if argv[1] == "logs" else "CREATE TABLE\n"
            Path(log).write_text(text)
            return 0, text
        skipped = {argv[index + 1] for index, word in enumerate(argv) if word == "--skip"}
        artifact = next(row for row in self.plan["artifacts"] if row["edition"] == env["YORISHIRO_TEST_EDITION"])
        names = sorted(set(artifact["tests"]) - skipped)
        if "--list" in argv:
            text = "".join(f"{name}: test\n" for name in names)
        else:
            text = "some fixture output\n" + "".join(f"interleaved log: test {name} ... ok\n" for name in names)
            text += f"test result: ok. {len(names)} passed; 0 failed; 0 ignored; 2 filtered out; finished in 0.01s\n"
        Path(log).write_text(text)
        return 0, text

    def execute(self, edition="ce", job="common-sqlite", run=None):
        return runner.execute_job(self.plan, edition, job, self.root / f"{edition}-{job}-logs", self.root / f"{edition}-{job}.json",
                                  self.manifest, self.env, "synthetic-revision", run_process=run or self.fake_process,
                                  binary_root=self.root)

    def receipts(self):
        return [self.execute("ce"), self.execute("ee")]

    def test_one_execution_per_target_not_per_test(self):
        receipt = self.execute()
        self.assertEqual(len(self.calls), 2)
        command = self.calls[1][0]
        self.assertIn("--exact", command)
        self.assertIn("--nocapture", command)
        self.assertEqual(command[command.index("--test-threads") + 1], "4")
        self.assertNotIn("--include-ignored", command)
        self.assertEqual({row["name"] for row in receipt["tests"]}, {"a", "b"})
        self.assertEqual(receipt["status"], "passed")

    def postgres(self):
        self.plan["jobs"]["common-sqlite"].update(database="postgres", queue="postgres")
        self.manifest["postgres_dedicated"] = [{"name": "b", "source": "tests/synthetic.rs"}]
        self.manifest["exceptions"] = [{"name": "b", "source": "tests/synthetic.rs"}]
        self.env.update(DATABASE_URL="postgres://localhost/yorishiro", QUEUE_URL="postgres://localhost/yorishiro",
                        LOCO_ENV="test_postgres", POSTGRES_CONTAINER="synthetic-postgres")

    def test_postgres_partitions_target_without_duplicate_tests(self):
        self.postgres()
        receipts = self.receipts()
        self.assertEqual([(row["fixture"], row["tests"]) for row in receipts[0]["binaries"]],
                         [("dedicated", ["b"]), ("shared", ["a"])])
        commands = [argv for argv, _, _ in self.calls]
        self.assertEqual(len([argv for argv in commands if "--nocapture" in argv]), 3)
        self.assertEqual(len([argv for argv in commands if argv[:2] == ["docker", "logs"]]), 2)
        self.assertEqual(runner.aggregate(self.plan, receipts, self.manifest)["tests_passed"], 3)
        shared = receipts[0]["binaries"][1]
        self.assertEqual(shared["ddl_observation"]["status"], "passed")
        shared.pop("ddl_observation")
        with self.assertRaisesRegex(ValueError, "DDL observation"):
            runner.aggregate(self.plan, receipts, self.manifest)

    def test_postgres_dedicated_manifest_rejects_duplicate_unknown_and_wrong_source(self):
        self.postgres()
        entry = self.manifest["postgres_dedicated"][0]
        for entries, message in (([entry, entry], "duplicate"),
                                 ([entry | {"name": "unknown"}], "unknown"),
                                 ([entry | {"source": "tests/wrong.rs"}], "source classification")):
            with self.subTest(entries=entries), self.assertRaisesRegex(ValueError, message):
                runner.postgres_dedicated(self.plan, self.manifest | {"postgres_dedicated": entries})

    def test_postgres_batches_multiple_shared_and_dedicated_identities(self):
        self.postgres()
        self.plan["artifacts"][0]["tests"].extend(["c", "d"])
        for name in ("c", "d"):
            self.plan["assignments"].append(self.plan["assignments"][0] | {"name": name})
        declaration = {"name": "d", "source": "tests/synthetic.rs"}
        self.manifest["postgres_dedicated"].append(declaration)
        self.manifest["exceptions"].append(declaration)
        receipt = self.execute()
        self.assertEqual([(row["fixture"], row["tests"]) for row in receipt["binaries"]],
                         [("dedicated", ["b", "d"]), ("shared", ["a", "c"])])
        self.assertEqual(len([argv for argv, _, _ in self.calls if "--nocapture" in argv]), 2)

    def test_shared_group_observes_first_process_boot_and_preserves_failure_logs(self):
        self.postgres()
        def failing(argv, env, deadline, log):
            skipped = {argv[index + 1] for index, word in enumerate(argv) if word == "--skip"}
            if "--nocapture" in argv and "b" in skipped:
                self.assertEqual(self.calls[-1][0][-1], runner.DDL_PROBE)
                Path(log).write_text("first OnceCell migration failed")
                return 17, "first OnceCell migration failed"
            return self.fake_process(argv, env, deadline, log)
        with self.assertRaisesRegex(ValueError, "libtest process failed"):
            self.execute(run=failing)
        receipt = runner.inventory.load(self.root / "ce-common-sqlite.json")
        shared = receipt["binaries"][-1]
        self.assertEqual(shared["exit_code"], 17)
        self.assertEqual(shared["ddl_observation"]["status"], "passed")
        self.assertEqual(receipt["status"], "failed")

    def test_shared_ddl_rejects_missing_probe_database_ddl_and_log_failure(self):
        for code, text in ((0, "no logging"), (0, runner.DDL_PROBE + "\nCREATE DATABASE bad;"),
                           (0, runner.DDL_PROBE + "\ndrop\n DATABASE bad;"), (1, runner.DDL_PROBE)):
            with self.subTest(code=code, text=text):
                evidence = {}
                def fake(argv, env, deadline, log, code=code, text=text):
                    result = (code, text) if argv[1] == "logs" else (0, "CREATE TABLE")
                    Path(log).write_text(result[1])
                    return result
                with (self.assertRaises(ValueError),
                      runner.shared_ddl_window({"POSTGRES_CONTAINER": "pg"}, runner.time.monotonic() + 30,
                                               self.root, "invalid", fake, evidence)):
                    pass
                self.assertEqual(evidence["status"], "failed")

    def test_shared_ddl_requires_container_and_successful_probe(self):
        with (self.assertRaisesRegex(ValueError, "POSTGRES_CONTAINER"),
              runner.shared_ddl_window({}, runner.time.monotonic() + 30, self.root, "missing",
                                       self.fake_process, {})):
            self.fail("must not execute without observation")
        def fake(argv, env, deadline, log):
            Path(log).write_text(runner.DDL_PROBE)
            return (1 if argv[1] == "exec" else 0), runner.DDL_PROBE
        evidence = {}
        with (self.assertRaisesRegex(ValueError, "positive probe failed"),
              runner.shared_ddl_window({"POSTGRES_CONTAINER": "pg"}, runner.time.monotonic() + 30,
                                       self.root, "probe", fake, evidence)):
            self.fail("must not execute after failed probe")
        self.assertEqual(evidence["probe_exit_code"], 1)
        self.assertEqual(evidence["status"], "failed")

    def test_shared_ddl_collects_logs_when_test_process_raises(self):
        evidence, calls = {}, []
        def fake(argv, env, deadline, log):
            calls.append(argv)
            Path(log).write_text(runner.DDL_PROBE)
            return 0, runner.DDL_PROBE
        with (self.assertRaisesRegex(ValueError, "test deadline"),
              runner.shared_ddl_window({"POSTGRES_CONTAINER": "pg"}, runner.time.monotonic() + 30,
                                       self.root, "timeout", fake, evidence)):
            raise ValueError("test deadline")
        self.assertEqual(calls[-1][:3], ["docker", "logs", "--since"])
        self.assertEqual(evidence["status"], "passed")

    def test_full_names_skipped_with_exact_not_substring(self):
        self.execute()
        command = self.calls[0][0]
        skipped = [command[index + 1] for index, word in enumerate(command) if word == "--skip"]
        self.assertEqual(skipped, ["a::nested", "helper"])

    def test_relocated_binary_hash_is_checked_before_listing(self):
        self.binary.write_text("tampered")
        with self.assertRaisesRegex(ValueError, "binary hash"):
            self.execute()
        self.assertFalse(self.calls)

    def test_artifact_symlink_escape_rejected(self):
        self.binary.unlink()
        self.binary.symlink_to("/dev/null")
        with self.assertRaisesRegex(ValueError, "escapes"):
            runner.relocated_binary(self.plan["artifacts"][0], self.root)

    def test_non_native_artifact_rejected(self):
        self.plan["artifacts"][0]["executable"] = "/build/target/aarch64/debug/deps/mod-synthetic"
        with self.assertRaisesRegex(ValueError, "non-native"):
            self.execute()
        self.assertFalse(self.calls)

    def test_wrong_topology_environment_rejected_before_process(self):
        mutations = [{"DATABASE_URL": "postgres://private@localhost/db"}, {"QUEUE_URL": "redis://localhost"},
                     {"LOCO_ENV": "test_postgres"}, {"YORISHIRO_LICENSE_KEY": "private-licence"},
                     {"YORISHIRO_TEST_EDITION": "ee"}, {"YORISHIRO_POSTGRES_TEST_URL": "postgres://localhost/db"},
                     {"YORISHIRO_REDIS_TEST_URL": "redis://localhost"}, {"YORISHIRO_TEST_QUEUE": "redis"},
                     {"YORISHIRO_QUEUE_KIND": "Redis"}]
        for mutation in mutations:
            with self.subTest(mutation=mutation):
                env = self.env | mutation
                with self.assertRaises(ValueError):
                    runner.topology_environment(self.plan["jobs"]["common-sqlite"], "ce", env)
        self.assertFalse(self.calls)

    def test_revision_rejected_before_process(self):
        self.plan["revision"] = "different"
        with self.assertRaisesRegex(ValueError, "revision"):
            self.execute()
        self.assertFalse(self.calls)

    def test_filtered_listing_must_match_before_execution(self):
        def wrong_listing(*args):
            return 0, "a: test\n"
        with self.assertRaisesRegex(ValueError, "listing differs"):
            self.execute(run=wrong_listing)
        receipt = runner.inventory.load(self.root / "ce-common-sqlite.json")
        self.assertEqual(receipt["status"], "failed")

    def test_missing_duplicate_failed_ignored_and_zero_results_rejected(self):
        for text in ("", "test a ... ok\ntest a ... ok\n", "test a ... FAILED\ntest b ... ok\n",
                     "test a ... ignored\ntest b ... ok\n", "test a ... ok\n"):
            with self.subTest(text=text), self.assertRaises(ValueError):
                runner.passed_tests(text, ["a", "b"])

    def test_interleaved_output_and_ansi_results(self):
        text = "fixture output test b ... \x1b[32mok\x1b[0m\nother output\ntest a ... ok\n"
        text += "test result: ok. 2 passed; 0 failed; 0 ignored; 5 filtered out; finished in 0.1s\n"
        self.assertEqual(runner.passed_tests(text, ["a", "b"]), ["a", "b"])

    def test_nocapture_can_interrupt_result_and_should_panic_is_a_pass(self):
        text = "test a - should panic ... provider log\nanother line\nok\ntest b ... ok\n"
        text += "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"
        self.assertEqual(runner.passed_tests(text, ["a", "b"]), ["a", "b"])

    def test_ignored_resource_permission_and_flag(self):
        artifact = self.plan["artifacts"][0]
        artifact["ignored"] = ["a"]
        rows = [self.plan["assignments"][0]]
        with self.assertRaisesRegex(ValueError, "resource permission"):
            runner.selection_flags(artifact, rows)
        rows[0].update(role="resource", include_ignored=True)
        self.assertIn("--include-ignored", runner.selection_flags(artifact, rows))

    def test_e5_resources_required_before_process(self):
        self.plan["jobs"]["resource-e5"] = self.plan["jobs"]["common-sqlite"]
        self.plan["assignments"][0].update(role="resource", jobs=["resource-e5"])
        with self.assertRaisesRegex(ValueError, "missing resource"):
            self.execute(job="resource-e5")
        self.assertFalse(self.calls)

    def test_complete_receipts_include_ee_delta(self):
        report = runner.aggregate(self.plan, self.receipts(), self.manifest)
        self.assertTrue(report["execution_matrix_ready"])
        self.assertEqual(report["tests_passed"], 3)

    def test_matrix_cannot_omit_provider_parity_cell(self):
        self.plan["jobs"]["queue-sqlite"] = self.plan["jobs"]["common-sqlite"]
        self.plan["assignments"][0]["jobs"].append("queue-sqlite")
        self.plan["execution_matrix"] = {"include": [
            {"edition": edition, "job": "common-sqlite", **self.plan["jobs"]["common-sqlite"]}
            for edition in ("ce", "ee")]}
        with self.assertRaisesRegex(ValueError, "matrix differs"):
            runner.provenance(self.plan, self.manifest)

    def test_verify_cli_discovers_nested_artifact_receipts(self):
        receipts = self.receipts()
        directory = self.root / "downloaded-receipts"
        for receipt in receipts:
            path = directory / ("artifact-" + receipt["edition"]) / "receipt.json"
            path.parent.mkdir(parents=True)
            runner.inventory.write(path, receipt)
        manifest, assignments, output = self.root / "manifest.json", self.root / "plan.json", self.root / "verified.json"
        runner.inventory.write(manifest, self.manifest)
        runner.inventory.write(assignments, self.plan)
        argv = ["run_assigned_tests.py", "verify", "--assignments", str(assignments),
                "--receipts", str(directory), "--output", str(output)]
        with patch.object(sys, "argv", argv), patch.object(runner.inventory, "MANIFEST", manifest):
            runner.main()
        self.assertEqual(runner.inventory.load(output)["tests_passed"], 3)

    def test_aggregation_rejects_missing_duplicate_wrong_or_failed_receipts(self):
        receipts = self.receipts()
        variants = [receipts[:1], receipts + [receipts[0]]]
        for field, value in (("edition", "ce"), ("status", "failed"), ("revision", "wrong"),
                             ("manifest_sha256", "wrong"), ("inventory_sha256", "wrong")):
            changed = copy.deepcopy(receipts)
            changed[1][field] = value
            variants.append(changed)
        for changed in variants:
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                runner.aggregate(self.plan, changed, self.manifest)

    def test_aggregation_rejects_wrong_hash_topology_skips_or_duplicate_tests(self):
        receipts = self.receipts()
        for field, value in (("sha256", "0" * 64), ("topology", {}), ("status", "ignored"), ("owner", "ee")):
            changed = copy.deepcopy(receipts)
            changed[0]["tests"][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                runner.aggregate(self.plan, changed, self.manifest)
        changed = copy.deepcopy(receipts)
        changed[0]["tests"].append(copy.deepcopy(changed[0]["tests"][0]))
        with self.assertRaisesRegex(ValueError, "duplicate test"):
            runner.aggregate(self.plan, changed, self.manifest)

    def test_pending_splits_block_success(self):
        self.plan["pending_splits"] = [{"name": "mixed"}]
        with self.assertRaisesRegex(ValueError, "pending scenario"):
            runner.aggregate(self.plan, self.receipts(), self.manifest)

    def test_helper_is_parent_assertion_not_independent_pass(self):
        self.plan["helpers"] = [{"package": "yorishiro", "target": "mod", "kind": "test", "name": "helper",
                                 "owner": "ee", "parent": "a", "requires_parent_child_receipt": True}]
        receipts = self.receipts()
        evidence = receipts[1]["helpers"][0]
        self.assertEqual(evidence["evidence"], "reviewed-parent-assertions")
        self.assertNotIn("helper", [row["name"] for row in receipts[1]["tests"]])
        self.assertEqual(runner.aggregate(self.plan, receipts, self.manifest)["tests_passed"], 3)
        receipts[1]["helpers"] = []
        with self.assertRaisesRegex(ValueError, "helper parent"):
            runner.aggregate(self.plan, receipts, self.manifest)

    def test_manual_external_disclosed_but_not_required_or_counted_passed(self):
        self.plan["jobs"]["resource-live"] = {"database": "sqlite", "queue": "sqlite", "status": "opt-in-not-required"}
        self.plan["assignments"].append(self.plan["assignments"][0] | {
            "name": "a::nested", "role": "manual_external", "jobs": ["resource-live"]})
        report = runner.aggregate(self.plan, self.receipts(), self.manifest)
        self.assertEqual(report["tests_passed"], 3)
        self.assertEqual(report["excluded_manual_external"][0]["status"], "opt-in-not-required")
        self.assertFalse(report["excluded_manual_external"][0]["counted_passed"])

    def test_manifest_optional_jobs_status_need_not_be_copied_into_plan(self):
        self.plan["jobs"]["resource-live"] = {"database": "sqlite", "queue": "sqlite"}
        self.manifest["optional_jobs"] = {"resource-live": {"status": "opt-in-not-required"}}
        self.plan["assignments"].append(self.plan["assignments"][0] | {
            "name": "a::nested", "role": "resource", "jobs": ["resource-live"]})
        report = runner.aggregate(self.plan, self.receipts(), self.manifest)
        self.assertEqual(report["tests_passed"], 3)
        self.assertFalse(report["excluded_manual_external"][0]["counted_passed"])

    def test_log_redaction(self):
        env = self.env | {"YORISHIRO_LICENSE_KEY": "private-licence", "API_TOKEN": "private-token"}
        text = "private-licence private-token postgres://user:pass@host/db Authorization: Bearer abc123"
        cleaned = runner.redact(text, env)
        for secret in ("private-licence", "private-token", "user:pass", "abc123"):
            self.assertNotIn(secret, cleaned)

    def test_timeout_cleans_up_process_group_and_redacts_log(self):
        class Child:
            pid = 12345
            returncode = -9

            def __init__(self):
                self.waits = 0

            def wait(self, timeout=None):
                self.waits += 1
                if timeout is not None:
                    raise runner.subprocess.TimeoutExpired("fake", timeout)

        child = Child()
        log = self.root / "timeout.log"
        def fake_popen(*args, **kwargs):
            kwargs["stdout"].write(b"Authorization: Bearer private-token\n")
            self.assertTrue(kwargs["start_new_session"])
            return child
        with (patch.object(runner.subprocess, "Popen", side_effect=fake_popen),
              patch.object(runner.os, "killpg") as kill,
              self.assertRaisesRegex(ValueError, "deadline exceeded")):
            runner.process(["never-executed"], self.env, runner.time.monotonic() + 10, log)
        kill.assert_called_once_with(child.pid, runner.signal.SIGKILL)
        self.assertEqual(child.waits, 2)
        self.assertNotIn("private-token", log.read_text())


if __name__ == "__main__":
    unittest.main()
